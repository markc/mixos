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
        let sender = header.sender().unwrap().as_str().to_owned();
        let token = String::try_from(options["handle_token"].try_clone().unwrap()).unwrap();
        let path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender.trim_start_matches(':').replace('.', "_"),
            token
        );
        connection
            .object_server()
            .at(path.as_str(), RequestObject(self.closed.clone()))
            .await
            .unwrap();
        if self.early == Some(99) {
            connection
                .object_server()
                .remove::<RequestObject, _>(path.as_str())
                .await
                .unwrap();
        } else if let Some(code) = self.early {
            emit(connection, &sender, &path, code).await;
        }
        self.calls
            .send(Call {
                method,
                options,
                path: path.clone(),
                owner: sender,
            })
            .unwrap();
        Ok(path.try_into().unwrap())
    }
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl Portal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
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
    let (_process, address) = dbus();
    let (_server, mut calls, closed) = serve(&address, Some(99)).await;
    let client = connection(&address).await;
    let (_tx, mut cancel) = watch::channel(false);
    let task = tokio::spawn(async move {
        portal::session_broken_stream(&client, &request("{}").unwrap(), &mut cancel, 2).await
    });
    let _call = tokio::time::timeout(Duration::from_secs(5), calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
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
