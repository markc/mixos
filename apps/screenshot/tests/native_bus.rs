// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Agent acceptance over a real broker: the headless binary, a fixture
//! compositor that only supplies deterministic pixels, and an agent that
//! captures, annotates, crops, saves into the screenshots folder (and hears
//! `screenshot.saved`), lists the recent shots, exports without clobbering,
//! is refused before unsaved work is discarded, cancels a delayed capture
//! and quits. Hardware capture is tested on the desktop.
use ::bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

const SERVICE: &str = "screenshot-test";
const COMP: &str = "comp-shot-test";

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn call(client: &SupervisedClient, verb: &str, args: Value) -> Result<Value, String> {
    client
        .call(SERVICE, verb, args)
        .await
        .map_err(|e| e.to_string())
}

/// The next body published on `topic`.
async fn next_on(
    incoming: &mut ::bus::native_client::BoundedIncomingReceiver,
    topic: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match incoming.recv().await {
                Some(BoundedIncomingEvent::Command(cmd)) if cmd.topic() == Some(topic) => {
                    return serde_json::from_str(&cmd.body).unwrap();
                }
                Some(_) => {}
                None => panic!("the subscription ended"),
            }
        }
    })
    .await
    .expect("an event within 10 s")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_capture_annotate_save_recent_export_cancel_and_quit() {
    let broker = test_broker::Broker::start();
    let directory = tempfile::tempdir().unwrap();
    let shots = directory.path().join("Pictures/Screenshots");

    // The agent, listening before anything is published.
    let client = Arc::new(
        SupervisedClient::connect_options("shot-agent", &broker.url)
            .bounded_incoming(64)
            .connect()
            .await
            .unwrap(),
    );
    let mut incoming = client.incoming_bounded().unwrap();
    for topic in [
        format!("{SERVICE}.saved"),
        format!("{SERVICE}.command.executed"),
    ] {
        client.subscribe_topic(&topic).await.unwrap();
    }

    // The fixture compositor: frames of one colour, no windows or outputs.
    let comp = Arc::new(
        SupervisedClient::connect_options(COMP, &broker.url)
            .connect()
            .await
            .unwrap(),
    );
    let mut requests = comp.incoming().unwrap();
    let server = comp.clone();
    let fixture = tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let args: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
            let (rc, body) = match request.command.as_str() {
                "comp.capture.frame" => {
                    let path = args["path"].as_str().unwrap().to_owned();
                    image::RgbaImage::from_pixel(120, 100, image::Rgba([230, 240, 250, 255]))
                        .save(&path)
                        .unwrap();
                    (
                        0,
                        json!({"path":path,"width":120,"height":100,"scale":1.0,"output":"test","source":"offscreen"}),
                    )
                }
                "comp.windows.list" => (0, json!({"windows":[]})),
                "comp.props.get" => (0, json!({"value":{}})),
                _ => (10, json!({"error":"the fixture handles frames only"})),
            };
            server
                .respond(&request, rc, &body.to_string())
                .await
                .unwrap();
        }
    });

    let _app = Process(
        Command::new(env!("CARGO_BIN_EXE_screenshot"))
            .args([
                "--headless",
                "--service",
                SERVICE,
                "--comp",
                COMP,
                "--noded-url",
                &broker.url,
            ])
            .env("HOME", directory.path())
            .env("MIXOS_APP_HOME", directory.path().join("state"))
            .env("MIXOS_SCREENSHOTS_DIR", &shots)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let info = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(info) = call(&client, "screenshot.info", json!({})).await {
                break info;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("screenshot registered");
    assert_eq!(info["headless"], true);

    // A second headless instance under the same name is refused.
    let duplicate = Command::new(env!("CARGO_BIN_EXE_screenshot"))
        .args([
            "--headless",
            "--service",
            SERVICE,
            "--noded-url",
            &broker.url,
        ])
        .env("MIXOS_APP_HOME", directory.path().join("duplicate"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!duplicate.success());

    // Capture, annotate, crop.
    let captured = call(&client, "screenshot.capture", json!({"cursor":false}))
        .await
        .unwrap();
    assert_eq!(captured["document"]["width"], 120);
    let source = PathBuf::from(captured["source"].as_str().unwrap());
    let original = std::fs::read(&source).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o600,
        "the raw capture is private"
    );
    let params = json!({"kind":"redact","points":[{"x":10,"y":10},{"x":50,"y":40}],"colour":[0,0,0,1],"width":2});
    let edit = call(
        &client,
        "screenshot.execute",
        json!({"id":"edit.annotate", "params":params}),
    )
    .await
    .unwrap();
    let alias = call(&client, "screenshot.annotate", params.clone())
        .await
        .unwrap();
    call(&client, "screenshot.execute", json!({"id":"edit.undo"}))
        .await
        .unwrap();
    let info = call(&client, "screenshot.info", json!({})).await.unwrap();
    assert_eq!(info["document"]["objects"].as_array().unwrap().len(), 1);
    assert_eq!(info["document"]["undo"], 1);
    for (id, object_id) in [
        ("edit.annotate", edit["result"]["id"].clone()),
        ("edit.annotate", alias["id"].clone()),
        ("edit.undo", Value::Null),
    ] {
        let event = next_on(&mut incoming, &format!("{SERVICE}.command.executed")).await;
        assert_eq!(event["v"], 2);
        assert_eq!(event["id"], id);
        assert_eq!(event["origin"], "bus");
        if id == "edit.annotate" {
            assert_eq!(event["params"], params);
            assert_eq!(event["result"]["id"], object_id);
        } else {
            assert!(event["params"].is_null());
        }
    }
    let refused = call(&client, "screenshot.open", json!({"path": source}))
        .await
        .unwrap_err();
    assert!(refused.contains("UNSAVED"), "{refused}");
    call(
        &client,
        "screenshot.crop",
        json!({"crop":{"x":10,"y":10,"width":80,"height":60}}),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(&source).unwrap(),
        original,
        "the capture is untouched by edits"
    );
    // Save into the folder: a timestamped name, the topic, the ledger.
    let saved = call(&client, "screenshot.save", json!({})).await.unwrap();
    let path = PathBuf::from(saved["saved"]["path"].as_str().unwrap());
    assert!(path.starts_with(&shots), "{path:?}");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(annotation::library::is_shot_name(&name), "{name}");
    let heard = next_on(&mut incoming, &format!("{SERVICE}.saved")).await;
    assert_eq!(heard["path"], json!(path));
    assert_eq!(heard["annotated"], true);
    assert_eq!(
        (heard["width"].clone(), heard["height"].clone()),
        (json!(80), json!(60))
    );
    assert!(heard["saved_at"].as_str().is_some_and(|s| s.len() >= 20));
    let image = image::open(&path).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (80, 60));
    assert_eq!(
        image.get_pixel(5, 5).0,
        [0, 0, 0, 255],
        "redaction is opaque"
    );
    assert!(!source.exists(), "the raw capture goes once it is saved");
    let recent = call(&client, "screenshot.recent", json!({"limit":5}))
        .await
        .unwrap();
    assert_eq!(recent["shots"][0]["path"], json!(path));
    assert_eq!(recent["folder"], json!(shots));

    // Save As publishes once and never replaces.
    let export = directory.path().join("export.png");
    call(&client, "screenshot.export", json!({"path": export}))
        .await
        .unwrap();
    let exported = std::fs::read(&export).unwrap();
    assert!(
        call(&client, "screenshot.export", json!({"path": export}))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&export).unwrap(), exported);

    // A save through the registry is published as an executed command.
    call(&client, "screenshot.annotate", json!({"kind":"arrow","points":[{"x":0,"y":0},{"x":30,"y":30}],"colour":[255,0,0,255],"width":3}))
        .await
        .unwrap();
    let executed = call(&client, "screenshot.execute", json!({"id":"file.save"}))
        .await
        .unwrap();
    assert_eq!(executed["executed"], "file.save");
    let run = next_on(&mut incoming, &format!("{SERVICE}.command.executed")).await;
    assert_eq!(run["id"], "edit.annotate");
    let run = next_on(&mut incoming, &format!("{SERVICE}.command.executed")).await;
    assert_eq!(run["id"], "file.save");
    assert_eq!(run["origin"], "bus");
    let second = next_on(&mut incoming, &format!("{SERVICE}.saved")).await;
    assert_ne!(second["path"], json!(path), "a new name, never the old one");

    // Open replaces a saved document; a delayed capture can be cancelled.
    call(&client, "screenshot.open", json!({"path": export}))
        .await
        .unwrap();
    let caller = client.clone();
    let pending =
        tokio::spawn(async move { call(&caller, "screenshot.capture", json!({"delay":10})).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while call(&client, "screenshot.info", json!({})).await.unwrap()["busy"] != true {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let cancel = call(&client, "screenshot.cancel", json!({})).await.unwrap();
    assert_eq!(cancel["cancelling"], true);
    let error = pending.await.unwrap().unwrap_err();
    assert!(error.contains("CANCELLED"), "{error}");
    let info = call(&client, "screenshot.info", json!({})).await.unwrap();
    assert_eq!(info["document"]["width"], 80, "the document is kept");
    assert_eq!(info["busy"], false);

    call(&client, "screenshot.quit", json!({})).await.unwrap();
    let mut app = _app;
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.0.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the app exits on quit");
    fixture.abort();
    comp.close().await;
    client.close().await;
}
