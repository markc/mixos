// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, atomic::AtomicUsize};
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

type Options = HashMap<String, OwnedValue>;
const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";

// Discovery lab only: one monotonic clock for fixture and client events.
fn portal_progress(event: impl std::fmt::Display) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    eprintln!("portal-call-progress +{:?} {event}", start.elapsed());
}

pub(super) struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn dbus() -> (Process, String) {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut address = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (Process(child), address.trim().into())
}

pub(super) async fn connection(address: &str) -> Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap()
}

struct RequestObject(Arc<AtomicUsize>);
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl RequestObject {
    fn close(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct RequestWithoutClose;
#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl RequestWithoutClose {
    fn ping(&self) -> bool {
        true
    }
}

// Lab only: GetAll represents enumeration waiting on an unrelated property.
// This is a controlled independent defect, not evidence of the historical race.
struct DelayedProperties {
    release: watch::Sender<bool>,
    entered: Arc<AtomicUsize>,
    entry: Arc<tokio::sync::Notify>,
    gets: Arc<AtomicUsize>,
}

// A persistent watch value also releases handlers that subscribe after teardown.
struct ReleaseProperties(watch::Sender<bool>);
impl Drop for ReleaseProperties {
    fn drop(&mut self) {
        portal_progress("cache gate released at teardown");
        self.0.send_replace(true);
    }
}

#[zbus::interface(name = "org.freedesktop.DBus.Properties")]
impl DelayedProperties {
    fn get(&self, interface: &str, property: &str) -> zbus::fdo::Result<OwnedValue> {
        if interface != "org.freedesktop.portal.FileChooser" {
            return Err(zbus::fdo::Error::UnknownInterface(interface.into()));
        }
        if property != "version" {
            return Err(zbus::fdo::Error::UnknownProperty(property.into()));
        }
        self.gets.fetch_add(1, Ordering::SeqCst);
        portal_progress("custom Properties.Get(version) responds immediately with 4");
        Ok(OwnedValue::from(4u32))
    }

    async fn get_all(&self, interface: &str) -> zbus::fdo::Result<Options> {
        if interface != "org.freedesktop.portal.FileChooser" {
            return Err(zbus::fdo::Error::UnknownInterface(interface.into()));
        }
        let mut release = self.release.subscribe();
        let entered = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
        portal_progress(format_args!(
            "custom Properties.GetAll entered #{entered}; waiting for unrelated property gate"
        ));
        self.entry.notify_one();
        release.wait_for(|released| *released).await.unwrap();
        portal_progress(format_args!("custom Properties.GetAll released #{entered}"));
        Ok(HashMap::from([
            ("version".into(), OwnedValue::from(4u32)),
            ("unrelated".into(), OwnedValue::from(0u32)),
        ]))
    }
}

struct Call {
    method: &'static str,
    options: Options,
    path: String,
    owner: String,
}
struct Portal {
    calls: tokio::sync::mpsc::UnboundedSender<Call>,
    closed: Arc<AtomicUsize>,
    early: Option<u32>,
}

impl Portal {
    async fn begin(
        &self,
        connection: &Connection,
        header: zbus::message::Header<'_>,
        method: &'static str,
        options: Options,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        if self.early == Some(99) {
            portal_progress(format_args!("fixture begin entered method={method}"));
        }
        let sender = header.sender().unwrap().as_str().to_owned();
        let token = String::try_from(options["handle_token"].try_clone().unwrap()).unwrap();
        let path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender.trim_start_matches(':').replace('.', "_"),
            token
        );
        if self.early == Some(99) {
            portal_progress(format_args!("fixture add start path={path}"));
        }
        let added = connection
            .object_server()
            .at(path.as_str(), RequestObject(self.closed.clone()))
            .await;
        if self.early == Some(99) {
            portal_progress(format_args!(
                "fixture add finished path={path} result={added:?}"
            ));
        }
        added.unwrap();
        if self.early == Some(99) {
            portal_progress(format_args!("fixture remove start path={path}"));
            let removed = connection
                .object_server()
                .remove::<RequestObject, _>(path.as_str())
                .await;
            portal_progress(format_args!(
                "fixture remove finished path={path} result={removed:?}"
            ));
            removed.unwrap();
        } else if let Some(code) = self.early
            && code != 98
        {
            emit(connection, &sender, &path, code).await;
        }
        if self.early == Some(99) {
            portal_progress(format_args!("fixture calls.send start path={path}"));
        }
        self.calls
            .send(Call {
                method,
                options,
                path: path.clone(),
                owner: sender,
            })
            .unwrap();
        if self.early == Some(99) {
            portal_progress(format_args!(
                "fixture calls.send finished; returning path={path}"
            ));
        }
        if self.early == Some(98) {
            std::future::pending::<()>().await;
        }
        Ok(path.try_into().unwrap())
    }
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl Portal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        if self.early == Some(99) {
            portal_progress("fixture version queried");
        }
        4
    }
    async fn open_file(
        &self,
        _parent: &str,
        _title: &str,
        options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.begin(connection, header, "OpenFile", options).await
    }
    async fn save_file(
        &self,
        _parent: &str,
        _title: &str,
        options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.begin(connection, header, "SaveFile", options).await
    }
    async fn save_files(
        &self,
        _parent: &str,
        _title: &str,
        options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.begin(connection, header, "SaveFiles", options).await
    }
}

async fn emit(connection: &Connection, owner: &str, path: &str, code: u32) {
    connection
        .emit_signal(
            Some(owner),
            path,
            REQUEST,
            "Response",
            &(code, Options::new()),
        )
        .await
        .unwrap();
}

async fn serve(
    address: &str,
    early: Option<u32>,
) -> (
    Connection,
    tokio::sync::mpsc::UnboundedReceiver<Call>,
    Arc<AtomicUsize>,
) {
    let connection = connection(address).await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let closed = Arc::new(AtomicUsize::new(0));
    connection
        .object_server()
        .at(
            PATH,
            Portal {
                calls: tx,
                closed: closed.clone(),
                early,
            },
        )
        .await
        .unwrap();
    connection.request_name(DESKTOP).await.unwrap();
    (connection, rx, closed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_hung_method_acknowledgement_is_bounded_and_final() {
    let (_process, address) = dbus();
    let (_server, mut calls, closed) = serve(&address, Some(98)).await;
    let client = connection(&address).await;
    let req = request("{}").unwrap();
    let (_cancel_tx, mut cancel) = watch::channel(false);
    let result = tokio::time::timeout(
        Duration::from_secs(7),
        portal::session(&client, &req, &mut cancel),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(result, Some(Outcome::Failed(message)) if message.contains("acknowledgement"))
    );
    assert!(
        calls.try_recv().is_ok(),
        "the portal actually opened its Request"
    );
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_maps_all_methods_and_subscribes_before_early_response() {
    let (_process, address) = dbus();
    let (_server, mut calls, _) = serve(&address, Some(1)).await;
    for (body, method) in [
        (
            json!({"mode":"open","multiple":true,"directory":true}),
            "OpenFile",
        ),
        (
            json!({"mode":"save","current_file":"file:///tmp/%FF.md"}),
            "SaveFile",
        ),
        (
            json!({"mode":"save_files","files":["a.md",{"bytes":"%FF.md"}]}),
            "SaveFiles",
        ),
    ] {
        let mut body = body;
        body["current_folder"] = json!("file:///tmp/%FE");
        body["accept_label"] = json!("Pick");
        body["choices"] = json!([{"id":"check","label":"Check","options":[]}]);
        if method != "SaveFiles" {
            body["filters"] = json!([{"name":"Text","patterns":["*.md"],"mime_types":["text/*"]}]);
            body["current_filter"] = json!({"name":"Other","patterns":["*.txt"],"mime_types":[]});
        }
        let request = request(&body.to_string()).unwrap();
        let client = connection(&address).await;
        let (_cancel_tx, mut cancel) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            portal::session(&client, &request, &mut cancel),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, Some(Outcome::Cancelled));
        let call = calls.recv().await.unwrap();
        assert_eq!(call.method, method);
        assert_eq!(
            String::try_from(call.options["accept_label"].try_clone().unwrap()).unwrap(),
            "Pick"
        );
        let folder =
            Vec::<u8>::try_from(call.options["current_folder"].try_clone().unwrap()).unwrap();
        assert_eq!(
            folder,
            if method == "SaveFile" {
                b"/tmp\0".to_vec()
            } else {
                b"/tmp/\xfe\0".to_vec()
            }
        );
        let choices = Vec::<(String, String, Vec<(String, String)>, String)>::try_from(
            call.options["choices"].try_clone().unwrap(),
        )
        .unwrap();
        assert_eq!(choices[0].3, "false");
        if method == "SaveFiles" {
            assert!(!call.options.contains_key("filters"));
            assert_eq!(
                Vec::<Vec<u8>>::try_from(call.options["files"].try_clone().unwrap()).unwrap(),
                [b"a.md\0".to_vec(), b"\xff.md\0".to_vec()]
            );
        } else {
            let filter = <(String, Vec<(u32, String)>)>::try_from(
                call.options["current_filter"].try_clone().unwrap(),
            )
            .unwrap();
            assert_eq!(filter.0, "Other");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_owner_skips_system_and_response_two_reaches_fallback() {
    let (_process, address) = dbus();
    let client = connection(&address).await;
    let req = request("{}").unwrap();
    let (_cancel_tx, mut cancel) = watch::channel(false);
    assert!(
        portal::session(&client, &req, &mut cancel)
            .await
            .unwrap()
            .is_none()
    );
    let (_server, _calls, _) = serve(&address, Some(2)).await;
    let (tx, rx) = mpsc::channel();
    let emitter = Emitter {
        tx,
        ctx: Context::default(),
        viewport: ViewportId::ROOT,
    };
    let mut system_cancel = cancel.clone();
    cascade(
        async { None },
        async {
            portal::session(&client, &req, &mut system_cancel)
                .await
                .unwrap()
        },
        cancel,
        &emitter,
    )
    .await;
    assert!(rx.try_iter().any(|m| matches!(m, Message::Fallback)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_stall_keeps_frames_running_and_drop_closes_request() {
    let (_process, address) = dbus();
    let (_server, mut calls, closed) = serve(&address, None).await;
    let mut handle = choose(request("{}").unwrap());
    handle.driver = Some(Box::new(move |request, cancel, emitter| {
        runtime().block_on(async {
            let client = connection(&address).await;
            let mut system_cancel = cancel.clone();
            cascade(
                async { None },
                async {
                    portal::session(&client, &request, &mut system_cancel)
                        .await
                        .unwrap()
                },
                cancel,
                &emitter,
            )
            .await;
        });
    }));
    let ctx = Context::default();
    frame(&ctx, ViewportId::ROOT, &mut handle);
    let _call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    // Exceed the connection/probe bound; an open chooser has no user deadline.
    let start = std::time::Instant::now();
    let mut frames = 0;
    while start.elapsed() < IO_BOUND + Duration::from_millis(100) {
        assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
        frames += 1;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(frames > 20);
    assert_eq!(handle.tier(), Tier::System);
    drop(handle);
    tokio::time::timeout(Duration::from_secs(5), async {
        while closed.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_response_zero_preserves_bytes_choices_and_filter() {
    let (_process, address) = dbus();
    let (server, mut calls, _) = serve(&address, None).await;
    let client = connection(&address).await;
    let req = request(r#"{"filters":[{"name":"Text","patterns":["*.md"],"mime_types":[]}],"choices":[{"id":"check","label":"Check","options":[]}]}"#).unwrap();
    let (_cancel_tx, mut cancel) = watch::channel(false);
    let task =
        tokio::spawn(async move { portal::session(&client, &req, &mut cancel).await.unwrap() });
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .expect("portal method was not called")
        .unwrap();
    let mut results = Options::new();
    results.insert(
        "uris".into(),
        OwnedValue::try_from(Value::from(vec!["file:///tmp/%FF.md"])).unwrap(),
    );
    results.insert(
        "choices".into(),
        OwnedValue::try_from(Value::from(vec![("check", "true")])).unwrap(),
    );
    results.insert(
        "current_filter".into(),
        OwnedValue::try_from(Value::from(("Text", vec![(0u32, "*.md")]))).unwrap(),
    );
    server
        .emit_signal(
            Some(call.owner.as_str()),
            call.path.as_str(),
            REQUEST,
            "Response",
            &(0u32, results),
        )
        .await
        .unwrap();
    let Some(Outcome::Chosen {
        paths,
        choices,
        current_filter,
    }) = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("portal outcome did not arrive")
        .unwrap()
    else {
        panic!("not chosen")
    };
    assert_eq!(path_bytes(&paths[0]), b"/tmp/\xff.md");
    assert_eq!(choices["check"], "true");
    assert_eq!(current_filter.unwrap().name, "Text");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_malformed_response_zero_is_final_in_the_handle() {
    let (_process, address) = dbus();
    let (_server, _calls, _) = serve(&address, Some(0)).await;
    let mut handle = choose(request("{}").unwrap());
    handle.driver = Some(Box::new(move |req, cancel, emitter| {
        runtime().block_on(async {
            let client = connection(&address).await;
            let mut system_cancel = cancel.clone();
            cascade(
                async { None },
                async {
                    portal::session(&client, &req, &mut system_cancel)
                        .await
                        .unwrap()
                },
                cancel,
                &emitter,
            )
            .await;
        });
    }));
    let ctx = Context::default();
    let outcome = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(answer) = frame(&ctx, ViewportId::ROOT, &mut handle) {
                break answer;
            }
            assert!(handle.fallback.is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(outcome, Outcome::Failed(_)));
    assert_eq!(handle.tier(), Tier::System);
}

#[test]
fn portal_selected_filter_is_matched_by_name_after_backend_normalisation() {
    let req = request(r#"{"filters":[{"name":"Text","patterns":["*.txt"],"mime_types":[]},{"name":"Images","patterns":["*.[pP][nN][gG]"],"mime_types":[]}]}"#).unwrap();
    let mut values = Options::new();
    values.insert(
        "uris".into(),
        OwnedValue::try_from(Value::from(vec!["file:///tmp/a.png"])).unwrap(),
    );
    // KDE normalises case-pair globs before returning the selected tuple.
    // This filter also differs from the request's initial current_filter.
    values.insert(
        "current_filter".into(),
        OwnedValue::try_from(Value::from(("Images", vec![(0u32, "*.png")]))).unwrap(),
    );
    let Outcome::Chosen { current_filter, .. } = portal::decode(&req, 0, &values).unwrap() else {
        panic!()
    };
    assert_eq!(current_filter, Some(req.filters[1].clone()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_dead_response_stream_is_rebuilt_once_before_close_recovery() {
    let (_process, address) = dbus();
    let (server, mut calls, closed) = serve(&address, None).await;
    let client = connection(&address).await;
    let (_tx, mut cancel) = watch::channel(false);
    let task = tokio::spawn(async move {
        portal::session_broken_stream(&client, &request("{}").unwrap(), &mut cancel, 1)
            .await
            .unwrap()
    });
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    emit(&server, &call.owner, &call.path, 1).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap(),
        Some(Outcome::Cancelled)
    );
    assert_eq!(closed.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_recovery_close_of_an_ended_request_falls_through() {
    portal_progress("test start");
    let (_process, address) = dbus();
    portal_progress("test private bus ready; serve start");
    let (_server, mut calls, closed) = serve(&address, Some(99)).await;
    portal_progress("test fixture registered and name acquired; client connection start");
    let client = connection(&address).await;
    portal_progress("test client connected");
    let (_tx, mut cancel) = watch::channel(false);
    let completion = Arc::new(std::sync::Mutex::new(String::from("not started")));
    let task_completion = completion.clone();
    let task = tokio::spawn(async move {
        *task_completion.lock().unwrap() = String::from("running");
        portal_progress("client task started");
        let result =
            portal::session_broken_stream(&client, &request("{}").unwrap(), &mut cancel, 2).await;
        let summary = format!("completed: {result:?}");
        portal_progress(format_args!("client task {summary}"));
        *task_completion.lock().unwrap() = summary;
        result
    });
    portal_progress("test calls.recv start (original 5s timeout)");
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv()).await;
    // Observe only: never poll/await the JoinHandle before its original assertion.
    portal_progress(format_args!(
        "test calls.recv finished timeout={} channel_closed={} client_finished={} client_state={} closed={}",
        call.is_err(),
        matches!(&call, Ok(None)),
        task.is_finished(),
        completion.lock().unwrap(),
        closed.load(Ordering::SeqCst)
    ));
    let _call = call.unwrap().unwrap();
    portal_progress("test client join start (original 8s timeout)");
    assert!(
        tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    portal_progress("test client join assertion passed");
    assert_eq!(closed.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_close_unknown_method_is_an_acknowledged_end() {
    let (_process, address) = dbus();
    let (server, _, _) = serve(&address, None).await;
    let path = "/org/freedesktop/portal/desktop/request/ended";
    server
        .object_server()
        .at(path, RequestWithoutClose)
        .await
        .unwrap();
    let client = connection(&address).await;
    let result = portal::close(&client, server.unique_name().unwrap().as_str(), path).await;
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_version_get_does_not_wait_for_unrelated_get_all() {
    let (_process, address) = dbus();
    let (server, mut calls, closed) = serve(&address, Some(99)).await;
    let (release, _) = watch::channel(false);
    let release_guard = ReleaseProperties(release.clone());
    let entered = Arc::new(AtomicUsize::new(0));
    let entry = Arc::new(tokio::sync::Notify::new());
    let gets = Arc::new(AtomicUsize::new(0));
    // Portal remains registered: removing the builtin must not destroy PATH.
    assert!(
        !server
            .object_server()
            .remove::<zbus::fdo::Properties, _>(PATH)
            .await
            .unwrap()
    );
    assert!(
        server
            .object_server()
            .at(
                PATH,
                DelayedProperties {
                    release: release.clone(),
                    entered: entered.clone(),
                    entry: entry.clone(),
                    gets: gets.clone(),
                },
            )
            .await
            .unwrap()
    );
    let probe = connection(&address).await;
    let owner = server.unique_name().unwrap().as_str().to_owned();
    let enumeration_probe = probe.clone();
    let enumeration_owner = owner.clone();
    let enumeration = tokio::spawn(async move {
        enumeration_probe
            .call_method(
                Some(enumeration_owner.as_str()),
                PATH,
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &("org.freedesktop.portal.FileChooser",),
            )
            .await
    });
    tokio::time::timeout(IO_BOUND, entry.notified())
        .await
        .expect("custom GetAll did not enter its gate");
    assert_eq!(entered.load(Ordering::SeqCst), 1);
    assert!(!enumeration.is_finished());
    // An actual wire Get, independent of proxy caching, validates the fixture.
    let reply = tokio::time::timeout(
        IO_BOUND,
        probe.call_method(
            Some(owner.as_str()),
            PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.FileChooser", "version"),
        ),
    )
    .await
    .expect("direct Get(version) was blocked by GetAll")
    .unwrap();
    assert_eq!(
        u32::try_from(reply.body().deserialize::<OwnedValue>().unwrap()).unwrap(),
        4
    );
    assert_eq!(gets.load(Ordering::SeqCst), 1);
    assert!(!*release.borrow());
    assert!(!enumeration.is_finished());
    portal_progress("fixture validated: wire Get(version)=4 while wire GetAll remains gated");

    let client = connection(&address).await;
    let (_tx, mut cancel) = watch::channel(false);
    let task = tokio::spawn(async move {
        portal_progress("controlled chooser started; production IO_BOUND remains 2s");
        let result =
            portal::session_broken_stream(&client, &request("{}").unwrap(), &mut cancel, 2).await;
        portal_progress(format_args!("controlled chooser completed: {result:?}"));
        result
    });
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv()).await;
    let result = tokio::time::timeout(Duration::from_secs(8), task).await;
    portal_progress(format_args!(
        "controlled observation: delivered={} client={result:?} GetAll_entries={} Get_calls={} gate_closed={}",
        matches!(&call, Ok(Some(_))),
        entered.load(Ordering::SeqCst),
        gets.load(Ordering::SeqCst),
        !*release.borrow()
    ));
    assert!(!*release.borrow());
    assert!(!enumeration.is_finished());
    // Release and reap the independent wire probe before outcome assertions,
    // including the expected old-client failure at the original 5s boundary.
    drop(release_guard);
    let values = tokio::time::timeout(IO_BOUND, enumeration)
        .await
        .expect("released GetAll did not finish")
        .unwrap()
        .unwrap()
        .body()
        .deserialize::<Options>()
        .unwrap();
    assert_eq!(
        u32::try_from(values["version"].try_clone().unwrap()).unwrap(),
        4
    );
    assert!(values.contains_key("unrelated"));
    let _call = call
        .expect("chooser method not delivered within original 5s: inspect cache causality markers")
        .unwrap();
    assert_eq!(gets.load(Ordering::SeqCst), 2, "chooser must read version");
    let result = result.unwrap().unwrap();
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().to_string(),
        "portal request cancelled after transport failure"
    );
    assert_eq!(closed.load(Ordering::SeqCst), 0);
}
