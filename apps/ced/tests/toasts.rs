// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Exercise the real Bus lane and late registration against noded.
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::bus::native_client::NodedClient;
use ced::app::App;
use ced::bus::{self, BusHandle, Delivery};
use documents::config::Config;
use documents::controller::{Controller, EDIT_UNAVAILABLE, Effect};
use editor_model::types::{Intent, Level, Notice};
use futures::StreamExt;

fn app() -> App {
    App::new(
        Controller::new(Config::default(), 1, false),
        Config::default(),
    )
}

fn flush(app: &mut App, bus: &BusHandle) {
    for effect in std::mem::take(&mut app.effects) {
        bus.perform(&effect);
    }
}

fn deliver(app: &mut App, delivery: Delivery) {
    match delivery {
        Delivery::Incoming(incoming) => {
            let fx = app.ctl.on_incoming(incoming);
            app.absorb(fx);
        }
        Delivery::NotificationFailed { text } => app.notification_failed(text),
        _ => {}
    }
}

#[tokio::test]
async fn warnings_and_errors_reach_notify_and_missing_notify_falls_back() {
    let broker = test_broker::Broker::start();
    let (handle, mut deliveries) = bus::spawn("ced", &broker.url, Arc::new(|| {})).unwrap();
    let mut app = app();
    app.absorb(vec![Effect::Notice {
        tab: None,
        notice: Notice::Message {
            level: Level::Error,
            text: "An error".into(),
        },
    }]);
    flush(&mut app, &handle);
    tokio::time::timeout(Duration::from_secs(4), async {
        while app.status() != Some("An error") {
            deliver(&mut app, deliveries.next().await.unwrap());
        }
    })
    .await
    .unwrap();

    let notify = NodedClient::connect("notify", &broker.url).await.unwrap();
    let mut requests = notify.incoming_async().await.unwrap();
    for level in [Level::Warn, Level::Error] {
        app.now += 6.0;
        app.absorb(vec![Effect::Notice {
            tab: None,
            notice: Notice::Message {
                level,
                text: "A warning or error".into(),
            },
        }]);
        flush(&mut app, &handle);
        let request = tokio::time::timeout(Duration::from_secs(3), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.command, "notify.send");
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["summary"], "Ced");
        assert_eq!(body["app"], "ced");
        assert_eq!(body["body"], "A warning or error");
        notify
            .respond(&request, 0, r#"{"id":1,"status":"open"}"#)
            .await
            .unwrap();
    }
    handle.shutdown(None);
    notify.close().await;
}

#[tokio::test]
async fn missing_edit_opens_automatically_after_late_registration() {
    let broker = test_broker::Broker::start();
    let (handle, mut deliveries) = bus::spawn("ced", &broker.url, Arc::new(|| {})).unwrap();
    let mut app = app();
    let fx = app.ctl.start();
    app.absorb(fx);
    let fx = app
        .ctl
        .open_paths(&["/work/hello.mix".into()], Intent::ui(0));
    app.absorb(fx);
    let tab = app.ctl.tabs()[0].id;
    let start = Instant::now();
    flush(&mut app, &handle);
    let mut subscribed = false;
    tokio::time::timeout(Duration::from_secs(4), async {
        while app.ctl.open_error(tab).is_none() || !subscribed {
            let delivery = deliveries.next().await.unwrap();
            if matches!(&delivery, Delivery::Subscribed { topic } if topic == "noded.props.changed")
            {
                subscribed = true;
            }
            deliver(&mut app, delivery);
            flush(&mut app, &handle);
        }
    })
    .await
    .unwrap();
    assert!(start.elapsed() < Duration::from_millis(3500));
    assert_eq!(app.ctl.open_error(tab), Some(EDIT_UNAVAILABLE));

    let edit = NodedClient::connect("edit", &broker.url).await.unwrap();
    let mut requests = edit.incoming_async().await.unwrap();
    let mut fake = editor_model::fake::FakeEditd::new("0000e1e1");
    fake.create(Some("/work/hello.mix"), "print(1)\n");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                delivery = deliveries.next() => {
                    deliver(&mut app, delivery.unwrap());
                    flush(&mut app, &handle);
                    if app.ctl.tabs()[0].mirror.as_ref().is_some_and(|m| matches!(m.phase(), editor_model::mirror::Phase::Live)) {
                        break;
                    }
                }
                request = requests.recv() => {
                    let request = request.unwrap();
                    let args = serde_json::from_str(&request.body).unwrap();
                    let answer = fake.handle("local:ced", &request.command, &args);
                    edit.respond(&request, answer.rc, &answer.body.to_string()).await.unwrap();
                }
            }
        }
    }).await.unwrap();
    assert!(app.ctl.open_error(tab).is_none());
    handle.shutdown(None);
    edit.close().await;
}
