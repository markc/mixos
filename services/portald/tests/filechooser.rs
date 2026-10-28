// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real D-Bus and noded with a scripted files provider. No display or user
//! manager is needed. Run on CBC: cargo test -p portald --test filechooser.
use bus::native_client::{IncomingCommand, SupervisedClient};
use futures_util::StreamExt;
use portald::filechooser::{INTERFACE, mapping::Options};
use portald::portal::{BUS_NAME, OBJECT_PATH};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const WAIT: Duration = Duration::from_secs(12);

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn connection(address: &str) -> Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap()
}

fn path(client: &Connection, token: &str) -> String {
    portald::screencast::handle::request_path(client.unique_name().unwrap().as_str(), token)
        .unwrap()
        .to_string()
}

async fn responses(client: &Connection, token: &str) -> MessageStream {
    MessageStream::for_match_rule(
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")
            .unwrap()
            .member("Response")
            .unwrap()
            .path(path(client, token))
            .unwrap()
            .build(),
        client,
        Some(16),
    )
    .await
    .unwrap()
}

async fn begin(
    client: &Connection,
    method: &str,
    token: &str,
    mut options: Options,
) -> zbus::Result<OwnedObjectPath> {
    options.insert(
        "handle_token".into(),
        OwnedValue::from(zbus::zvariant::Str::from(token)),
    );
    client
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            method,
            &("wayland:parent", "test", options),
        )
        .await?
        .body()
        .deserialize()
}

async fn close(client: &Connection, request: &str) -> zbus::Result<()> {
    client
        .call_method(
            Some(BUS_NAME),
            request,
            Some("org.freedesktop.portal.Request"),
            "Close",
            &(),
        )
        .await?;
    Ok(())
}

async fn response(stream: &mut MessageStream, code: u32) -> HashMap<String, OwnedValue> {
    let message = tokio::time::timeout(WAIT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (actual, results): (u32, HashMap<String, OwnedValue>) =
        message.body().deserialize().unwrap();
    assert_eq!(actual, code);
    results
}

async fn quiet(stream: &mut MessageStream) {
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "unexpected duplicate/closed Response"
    );
}

enum Control {
    Finish(String, u32, bool),
    Release,
    Incarnation,
    Forget(String),
}

struct Provider {
    client: Arc<SupervisedClient>,
    task: tokio::task::JoinHandle<()>,
    control: mpsc::UnboundedSender<Control>,
    chosen: mpsc::UnboundedReceiver<(String, Json)>,
    cancelled: mpsc::UnboundedReceiver<String>,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Provider {
    async fn start(url: &str) -> Self {
        let client = Arc::new(
            SupervisedClient::connect_options("files", url)
                .with_commands(
                    [
                        "files.choose",
                        "files.choose.list",
                        "files.choose.cancel",
                        "files.choose.result",
                    ]
                    .into_iter()
                    .map(|s| bus::CommandDescriptor::new(s, &[], "test", false))
                    .collect(),
                )
                .connect()
                .await
                .unwrap(),
        );
        let mut incoming = client.incoming().unwrap();
        let (control, mut controls) = mpsc::unbounded_channel();
        let (chosen_tx, chosen) = mpsc::unbounded_channel();
        let (cancel_tx, cancelled) = mpsc::unbounded_channel();
        let c = client.clone();
        let task = tokio::spawn(async move {
            let mut sequence = 0;
            let mut incarnation = "a";
            let mut open: HashMap<String, Json> = HashMap::new();
            let mut done: HashMap<String, Json> = HashMap::new();
            let mut held: Vec<(IncomingCommand, Json)> = Vec::new();
            loop {
                tokio::select! {
                    command = incoming.recv() => {
                        let Some(command) = command else { break };
                        let body: Json = serde_json::from_str(&command.body).unwrap_or(json!({}));
                        let mut rc = 0;
                        let answer = match command.command.as_str() {
                            "files.choose.list" => json!({"incarnation":incarnation,"requests":open.iter().map(|(id, b)|json!({"request":id,"correlation":b["correlation"]})).collect::<Vec<_>>()}),
                            "files.choose" => {
                                if body["accept_label"] == "BUSY" || body["accept_label"] == "ARGUMENT" || body["accept_label"] == "UNAVAILABLE" {
                                    rc = 10; json!({"error_code":body["accept_label"]})
                                } else {
                                    sequence += 1;
                                    let id = format!("{incarnation}-{sequence}");
                                    let reply = json!({"request":id,"topic":"files.chosen","correlation":body["correlation"]});
                                    open.insert(id.clone(), body.clone());
                                    chosen_tx.send((id.clone(), body.clone())).unwrap();
                                    if body["accept_label"] == "early" {
                                        let outcome = json!({"request":id,"correlation":body["correlation"],"response":0,"uris":["file:///tmp/early"]});
                                        publish(&c, &outcome).await;
                                        open.remove(&id); done.insert(id, outcome);
                                    }
                                    if body["accept_label"] == "hold" { held.push((command, reply)); continue; }
                                    reply
                                }
                            }
                            "files.choose.cancel" => {
                                let id = body["request"].as_str().unwrap().to_owned();
                                open.remove(&id); let _ = cancel_tx.send(id); json!({})
                            }
                            "files.choose.result" => {
                                let id = body["request"].as_str().unwrap();
                                if let Some(value) = done.get(id) { json!({"state":"done","outcome":value}) }
                                else if open.contains_key(id) { json!({"state":"open"}) }
                                else { rc = 10; json!({"reason":"unknown"}) }
                            }
                            _ => continue,
                        };
                        c.respond(&command, rc, &answer.to_string()).await.unwrap();
                    }
                    control = controls.recv() => {
                        match control {
                            Some(Control::Finish(id, response, emit)) => {
                                let body = open.remove(&id).unwrap();
                                let outcome = json!({"request":id,"correlation":body["correlation"],"response":response,
                                    "uris":["file:///tmp/%FF"],"choices":{"check":"true"},"current_filter":{"name":"Text","patterns":["*.txt"],"mime_types":[]}});
                                if emit { publish(&c, &outcome).await; }
                                done.insert(id, outcome);
                            }
                            Some(Control::Release) => {
                                for (command, reply) in held.drain(..) { c.respond(&command, 0, &reply.to_string()).await.unwrap(); }
                            }
                            Some(Control::Incarnation) => { incarnation = "b"; open.clear(); done.clear(); }
                            Some(Control::Forget(id)) => { open.remove(&id); done.remove(&id); }
                            None => break,
                        }
                    }
                }
            }
        });
        Self {
            client,
            task,
            control,
            chosen,
            cancelled,
        }
    }

    async fn chosen(&mut self) -> (String, Json) {
        tokio::time::timeout(WAIT, self.chosen.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn cancelled(&mut self, expected: &str) {
        let actual = tokio::time::timeout(WAIT, self.cancelled.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
    }
}

async fn publish(client: &SupervisedClient, outcome: &Json) {
    let mut frame = bus::BusMessage::new();
    frame.set("command", "files.chosen");
    frame.body = outcome.to_string();
    let headers = BTreeMap::from([
        ("name".into(), "files.chosen".into()),
        ("retain".into(), "false".into()),
    ]);
    let (rc, body, _) = client
        .call_with_headers_raw("noded", "topic.publish", &headers, &frame.to_wire())
        .await
        .unwrap();
    assert_eq!(rc, 0, "{body}");
}

fn label(label: &str) -> Options {
    HashMap::from([(
        "accept_label".into(),
        OwnedValue::from(zbus::zvariant::Str::from(label)),
    )])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_native_stack_mapping_lifecycle_and_loss() {
    let dir = tempfile::tempdir().unwrap();
    let mut dbus = Process(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .arg(format!("--address=unix:path={}/bus", dir.path().display()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(dbus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut broker = test_broker::Broker::with_tcp_port(port);
    let mut provider = Provider::start(&broker.url).await;
    provider.client.call("files", "files.choose", json!({"correlation":"/org/freedesktop/portal/desktop/request/old/startup"})).await.unwrap();
    let stale = provider.chosen().await.0;
    let (native, incoming) = portald::service::connect(&broker.url).unwrap();
    let bridge = portald::filechooser::Bridge::new(native.clone());
    let (outcomes, outcome_rx) = mpsc::channel(64);
    let (projections, received) = mpsc::channel(8);
    let status = portald::status::shared();
    let binding = settings::Binding {
        instance: "test".into(),
        profile: "default".into(),
    };
    let feed = tokio::spawn(portald::service::run(
        binding.clone(),
        native.clone(),
        portald::filechooser::Source { incoming, outcomes },
        projections,
        status.clone(),
    ));
    let portal = tokio::spawn(portald::live::serve(
        portald::live::Config {
            address: address.trim().into(),
            state_dir: None,
            binding,
            screencast: None,
            screencast_max_framerate: Default::default(),
            gtk_settings: portald::gtk_settings::Exporter::new(None, None),
            filechooser: Some((bridge, outcome_rx)),
        },
        received,
        status,
    ));
    let caller = connection(address.trim()).await;
    let stranger = connection(address.trim()).await;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(reply) = caller
                .call_method(
                    Some(BUS_NAME),
                    OBJECT_PATH,
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &(INTERFACE, "version"),
                )
                .await
            {
                assert_eq!(
                    u32::try_from(reply.body().deserialize::<OwnedValue>().unwrap()).unwrap(),
                    4
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    provider.cancelled(&stale).await;

    for (method, mode) in [
        ("OpenFile", "open"),
        ("SaveFile", "save"),
        ("SaveFiles", "save_files"),
    ] {
        let mut stream = responses(&caller, "same").await;
        let mut options = Options::new();
        if mode == "save_files" {
            options.insert(
                "files".into(),
                OwnedValue::try_from(Value::from(vec![b"a\0".to_vec()])).unwrap(),
            );
        }
        let request = begin(&caller, method, "same", options).await.unwrap();
        let (id, body) = provider.chosen().await;
        assert_eq!(body["mode"], mode);
        assert_eq!(body["correlation"], request.as_str());
        assert_eq!(body["parent_window"], "wayland:parent");
        assert!(
            body["on_behalf_of"]
                .as_str()
                .unwrap()
                .starts_with(caller.unique_name().unwrap().as_str())
        );
        provider.control.send(Control::Finish(id, 0, true)).unwrap();
        let results = response(&mut stream, 0).await;
        assert_eq!(
            Vec::<String>::try_from(results["uris"].try_clone().unwrap()).unwrap(),
            vec!["file:///tmp/%FF"]
        );
        quiet(&mut stream).await;
    }

    // Early publication, provider refusal, and an absent provider all answer
    // through Response rather than a method error.
    for (accept, code) in [
        ("early", 0),
        ("BUSY", 2),
        ("ARGUMENT", 2),
        ("UNAVAILABLE", 2),
    ] {
        let mut stream = responses(&caller, "early").await;
        begin(&caller, "OpenFile", "early", label(accept))
            .await
            .unwrap();
        if accept == "early" {
            provider.chosen().await;
        }
        response(&mut stream, code).await;
        quiet(&mut stream).await;
    }

    // Three applications can use the same token concurrently; only their
    // addressed signal reaches each client. Another owner cannot Close it.
    let third = connection(address.trim()).await;
    let mut streams = Vec::new();
    for client in [&caller, &stranger, &third] {
        let stream = responses(client, "parallel").await;
        begin(client, "OpenFile", "parallel", Options::new())
            .await
            .unwrap();
        streams.push(stream);
    }
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(provider.chosen().await.0);
    }
    let err = close(&stranger, &path(&caller, "parallel"))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("org.freedesktop.portal.Error.NotAllowed"),
        "{err}"
    );
    assert!(
        begin(&caller, "OpenFile", "parallel", Options::new())
            .await
            .is_err()
    );
    for id in ids {
        provider.control.send(Control::Finish(id, 1, true)).unwrap();
    }
    for stream in &mut streams {
        assert!(response(stream, 1).await.is_empty());
        quiet(stream).await;
    }

    // Close while open, while sending, and immediately reuse the same token.
    for held in [false, true] {
        let mut stream = responses(&caller, "reuse").await;
        begin(
            &caller,
            "OpenFile",
            "reuse",
            if held { label("hold") } else { Options::new() },
        )
        .await
        .unwrap();
        let old = provider.chosen().await.0;
        close(&caller, &path(&caller, "reuse")).await.unwrap();
        if !held {
            provider.cancelled(&old).await;
        }
        begin(&caller, "SaveFile", "reuse", Options::new())
            .await
            .unwrap();
        let new = provider.chosen().await.0;
        if held {
            provider.control.send(Control::Release).unwrap();
            provider.cancelled(&old).await;
        }
        provider
            .control
            .send(Control::Finish(new, 0, true))
            .unwrap();
        response(&mut stream, 0).await;
        quiet(&mut stream).await;
    }

    // Disconnecting a persistent D-Bus caller cancels its native window.
    let leaving = connection(address.trim()).await;
    begin(&leaving, "OpenFile", "leave", Options::new())
        .await
        .unwrap();
    let id = provider.chosen().await.0;
    leaving.close().await.unwrap();
    provider.cancelled(&id).await;

    let leaving = connection(address.trim()).await;
    begin(&leaving, "OpenFile", "leave_sending", label("hold")).await.unwrap();
    let id = provider.chosen().await.0;
    leaving.close().await.unwrap();
    provider.control.send(Control::Release).unwrap();
    provider.cancelled(&id).await;

    // A missed publication is recovered from result; an unknown result fails.
    for known in [true, false] {
        let mut stream = responses(&caller, "missed").await;
        begin(&caller, "OpenFile", "missed", Options::new())
            .await
            .unwrap();
        let id = provider.chosen().await.0;
        provider
            .control
            .send(if known {
                Control::Finish(id, 0, false)
            } else {
                Control::Forget(id)
            })
            .unwrap();
        response(&mut stream, if known { 0 } else { 2 }).await;
        quiet(&mut stream).await;
    }

    let mut stream = responses(&caller, "timeout").await;
    begin(&caller, "OpenFile", "timeout", label("hold"))
        .await
        .unwrap();
    let id = provider.chosen().await.0;
    response(&mut stream, 2).await;
    provider.control.send(Control::Release).unwrap();
    provider.cancelled(&id).await;
    quiet(&mut stream).await;

    // Losing the native Bus is final for the old Request. A reconnect
    // resubscribes and clears its leftover chooser before accepting new ones.
    let mut stream = responses(&caller, "bus_loss").await;
    begin(&caller, "OpenFile", "bus_loss", Options::new()).await.unwrap();
    let id = provider.chosen().await.0;
    broker.bounce();
    response(&mut stream, 2).await;
    provider.cancelled(&id).await;
    let mut stream = responses(&caller, "after_reconnect").await;
    begin(&caller, "OpenFile", "after_reconnect", label("early")).await.unwrap();
    provider.chosen().await;
    response(&mut stream, 0).await;

    // Incarnation changes fail all requests from the old provider.
    let mut stream = responses(&caller, "loss").await;
    begin(&caller, "OpenFile", "loss", Options::new())
        .await
        .unwrap();
    provider.chosen().await;
    provider.control.send(Control::Incarnation).unwrap();
    response(&mut stream, 2).await;
    let mut stream = responses(&caller, "killed").await;
    begin(&caller, "OpenFile", "killed", Options::new()).await.unwrap();
    provider.chosen().await;
    provider.client.close().await;
    response(&mut stream, 2).await;
    let mut stream = responses(&caller, "absent").await;
    begin(&caller, "OpenFile", "absent", Options::new())
        .await
        .unwrap();
    response(&mut stream, 2).await;

    portal.abort();
    feed.abort();
    native.close().await;
}
