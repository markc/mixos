// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use bus::native_client::SupervisedClient;
use std::sync::{Arc, Mutex, atomic::AtomicUsize};

#[test]
fn worker_death_opens_fallback_and_keeps_the_request() {
    let req = request(r#"{"title":"Recover this request","mode":"save","current_name":"report","current_filter":{"name":"Text","patterns":["*.md"],"mime_types":[]}}"#).unwrap();
    let mut handle = choose(req);
    handle.driver = Some(Box::new(|_, _, _| {}));
    let ctx = Context::default();
    frame(&ctx, ViewportId::ROOT, &mut handle);
    let saved = handle.request.clone();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while handle.tier() != Tier::InProcess && std::time::Instant::now() < until {
        assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
        std::thread::yield_now();
    }
    assert_eq!(handle.tier(), Tier::InProcess);
    assert_eq!(handle.request, saved);
    let first = handle.fallback.as_ref().unwrap() as *const _;
    frame(&ctx, ViewportId::ROOT, &mut handle);
    assert_eq!(first, handle.fallback.as_ref().unwrap() as *const _);
    handle.cancel();
    assert_eq!(
        frame(&ctx, ViewportId::ROOT, &mut handle),
        Some(Outcome::Cancelled)
    );
    assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
}

#[test]
fn two_failed_tiers_open_the_in_process_chooser_with_the_same_request() {
    let mut handle = choose(request(r#"{"mode":"save_files","title":"Keep this title","files":["one.md","two.md"],"choices":[{"id":"check","label":"Check","options":[],"default":"true"}]}"#).unwrap());
    assert!(
        handle.request.correlation.is_some(),
        "the same request must remain identifiable through the handoff"
    );
    handle.driver = Some(Box::new(|_, cancel, emitter| {
        runtime().block_on(cascade(async { None }, async { None }, cancel, &emitter));
    }));
    #[cfg(all(target_os = "linux", feature = "portal"))]
    let _private = {
        let broker = test_broker::Broker::start();
        let (process, address) = super::portal_tests::dbus();
        let url = broker.url.clone();
        handle.driver = Some(Box::new(move |req, cancel, emitter| {
            runtime().block_on(async {
                let client = super::portal_tests::connection(&address).await;
                let mut bus_cancel = cancel.clone();
                let mut portal_cancel = cancel.clone();
                cascade(native::attempt_at(&req, &mut bus_cancel, &url), async {
                    portal::session(&client, &req, &mut portal_cancel).await.unwrap()
                }, cancel, &emitter).await;
            });
        }));
        (broker, process)
    };
    let ctx = Context::default();
    frame(&ctx, ViewportId::ROOT, &mut handle);
    let saved = handle.request.clone();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while handle.tier() != Tier::InProcess && std::time::Instant::now() < until {
        frame(&ctx, ViewportId::ROOT, &mut handle);
        std::thread::yield_now();
    }
    assert_eq!(handle.tier(), Tier::InProcess);
    assert_eq!(handle.request, saved);
    let Outcome::Chosen { choices, .. } = handle.fallback.as_ref().unwrap().chosen(vec![]) else {
        panic!()
    };
    assert_eq!(choices["check"], "true");
}

#[test]
fn edited_request_is_a_final_validation_error_without_a_worker() {
    let mut req = request("{}").unwrap();
    req.current_folder = Some(b"relative\0".to_vec());
    let mut handle = choose(req);
    let ctx = Context::default();
    assert!(matches!(
        frame(&ctx, ViewportId::ROOT, &mut handle),
        Some(Outcome::Failed(_))
    ));
    assert!(handle.receiver.is_none());
    assert!(handle.fallback.is_none());
}

#[test]
fn app_id_is_forwarded() {
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let child = ViewportId::from_hash_of("app-id-test");
    ctx.run_ui(Default::default(), |ui| {
        ui.ctx().show_viewport_deferred(
            child,
            egui::ViewportBuilder::default().with_app_id("dev.mixos.example"),
            |_, _| {},
        );
    })
    .drop_without_applying_deltas();
    let mut one = choose(request(r#"{"correlation":"app-id-test"}"#).unwrap());
    let (tx, rx) = mpsc::channel();
    one.driver = Some(Box::new(move |req, _, emitter| {
        tx.send(req).unwrap();
        emitter.send(Message::Answer(Outcome::Cancelled));
    }));
    frame(&ctx, child, &mut one);
    let forwarded = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(forwarded.on_behalf_of, crate::titlebar::app_id().or_else(|| Some("dev.mixos.example".into())));
    assert_eq!(forwarded.correlation, one.request.correlation);
}

#[test]
fn absent_correlation_is_unique_per_handle() {
    let one = choose(request("{}").unwrap());
    let two = choose(request("{}").unwrap());
    assert!(one.request.correlation.is_some());
    assert_ne!(one.request.correlation, two.request.correlation);
}

#[test]
fn platform_save_result_uses_the_contract_extension_rule() {
    let req = request(
        r#"{"mode":"save","current_filter":{"name":"Text","patterns":["*.md"],"mime_types":[]}}"#,
    )
    .unwrap();
    for (name, expected) in [
        ("report", "report.md"),
        ("report.", "report.md"),
        ("report.txt", "report.txt"),
    ] {
        let Outcome::Chosen { paths, .. } =
            platform_paths(&req, vec![PathBuf::from("/tmp").join(name)])
        else {
            panic!()
        };
        assert_eq!(paths, [PathBuf::from("/tmp").join(expected)]);
    }
}

#[test]
fn malformed_accept_and_any_cancel_end_the_cascade_without_another_tier() {
    for value in [
        json!({"response":0,"uris":[]}),
        json!({"response":1,"reason":"caller"}),
        json!({"response":1,"reason":"other"}),
    ] {
        let answer = decode_outcome(&request("{}").unwrap(), &value);
        let (tx, rx) = mpsc::channel();
        let emitter = Emitter {
            tx,
            ctx: Context::default(),
            viewport: ViewportId::ROOT,
        };
        let (_tx, cancel) = watch::channel(false);
        runtime().block_on(cascade(
            async { Some(answer.clone()) },
            async { panic!("final answer advanced") },
            cancel,
            &emitter,
        ));
        let messages: Vec<_> = rx.try_iter().collect();
        assert!(matches!(messages.last(), Some(Message::Answer(a)) if a == &answer));
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, Message::Fallback | Message::Tier(Tier::System)))
        );
    }
}

struct Fake {
    client: Arc<SupervisedClient>,
    task: tokio::task::JoinHandle<()>,
    opened: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    state: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    started: tokio::sync::mpsc::UnboundedReceiver<()>,
    queried: Arc<tokio::sync::Notify>,
}

impl Fake {
    // 0: open; 1: restart; 2: list fails; 3: lost ack; 4: error after open;
    // 5: external caller cancelled; 6: malformed accepted answer;
    // 8: acknowledgement arrived without its request id.
    async fn start(url: &str, state: usize) -> Self {
        let client = Arc::new(
            SupervisedClient::connect_options("files", url)
                .with_commands(
                    files::chooser::COMMANDS
                        .iter()
                        .map(|(name, query)| bus::CommandDescriptor::new(name, &[], "test", *query))
                        .collect(),
                )
                .connect()
                .await
                .unwrap(),
        );
        let mut incoming = client.incoming().unwrap();
        let opened = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(AtomicUsize::new(state));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (tx, started) = tokio::sync::mpsc::unbounded_channel();
        let queried = Arc::new(tokio::sync::Notify::new());
        let q = queried.clone();
        let (p, o, c, s, r) = (
            client.clone(),
            opened.clone(),
            closed.clone(),
            state.clone(),
            requests.clone(),
        );
        let task = tokio::spawn(async move {
            while let Some(command) = incoming.recv().await {
                let body: serde_json::Value =
                    serde_json::from_str(&command.body).unwrap_or(json!({}));
                let mode = s.load(Ordering::SeqCst);
                let mut rc = 0;
                let value = match command.command.as_str() {
                    "files.choose" => {
                        o.fetch_add(1, Ordering::SeqCst);
                        r.lock().unwrap().push(json!({"request":"0123456789abcdef-1", "caller":"a real broker supplied identity", "correlation":body["correlation"]}));
                        tx.send(()).unwrap();
                        if mode == 3 {
                            continue;
                        }
                        if mode == 4 {
                            rc = 10;
                            json!({"error_code":"BROKEN","message":"reply failed after opening"})
                        } else if mode == 8 {
                            json!({})
                        } else {
                            json!({"request":"0123456789abcdef-1"})
                        }
                    }
                    "files.choose.list" => {
                        if mode == 2 && o.load(Ordering::SeqCst) > 0 {
                            rc = 10;
                            json!({"error_code":"UNAVAILABLE"})
                        } else {
                            json!({"incarnation":if mode == 1 {"fedcba9876543210"} else {"0123456789abcdef"}, "requests":*r.lock().unwrap()})
                        }
                    }
                    "files.choose.cancel" => {
                        c.fetch_add(1, Ordering::SeqCst);
                        r.lock().unwrap().clear();
                        json!({})
                    }
                    "files.choose.result" if mode == 5 => {
                        json!({"state":"done","outcome":{"request":"0123456789abcdef-1","response":1,"reason":"caller"}})
                    }
                    "files.choose.result" if mode == 6 => {
                        json!({"state":"done","outcome":{"request":"0123456789abcdef-1","response":0,"uris":[]}})
                    }
                    _ => {
                        q.notify_one();
                        json!({"state":"open"})
                    }
                };
                p.respond(&command, rc, &value.to_string()).await.unwrap();
            }
        });
        Self {
            client,
            task,
            opened,
            closed,
            state,
            requests,
            started,
            queried,
        }
    }

    async fn stop(&self) {
        self.task.abort();
        self.client.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_provider_and_broker_loss_open_the_next_tier_once_within_fifteen_seconds() {
    for (loss, sending) in [
        (0, false),
        (1, false),
        (2, false),
        (3, false),
        (0, true),
        (1, true),
    ] {
        let mut broker = test_broker::Broker::start();
        let mut fake = Fake::start(&broker.url, if sending { 3 } else { 0 }).await;
        let req = request("{}").unwrap();
        let (_tx, mut cancel) = watch::channel(false);
        let url = broker.url.clone();
        let (tx, rx) = mpsc::channel();
        let emitter = Emitter {
            tx,
            ctx: Context::default(),
            viewport: ViewportId::ROOT,
        };
        let (_cascade_tx, cascade_cancel) = watch::channel(false);
        let task = tokio::spawn(async move {
            cascade(
                native::attempt_at(&req, &mut cancel, &url),
                async { Some(Outcome::Cancelled) },
                cascade_cancel,
                &emitter,
            )
            .await;
        });
        tokio::time::timeout(Duration::from_secs(5), fake.started.recv())
            .await
            .unwrap()
            .unwrap();
        if !sending {
            tokio::time::timeout(Duration::from_secs(5), fake.queried.notified())
                .await
                .unwrap();
        }
        match loss {
            0 => fake.stop().await,
            1 => broker.stop(),
            2 => {
                fake.state.store(1, Ordering::SeqCst);
            }
            _ => {
                fake.state.store(2, Ordering::SeqCst);
            }
        }
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("lost provider blocked fallback")
            .unwrap();
        let messages: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            messages
                .iter()
                .filter(|m| matches!(m, Message::Tier(Tier::System)))
                .count(),
            1
        );
        assert_eq!(
            messages
                .iter()
                .filter(|m| matches!(m, Message::Answer(Outcome::Cancelled)))
                .count(),
            1
        );
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        fake.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_dropped_or_error_reply_cancels_our_correlation_before_fallback() {
    for mode in [3, 4] {
        let broker = test_broker::Broker::start();
        let fake = Fake::start(&broker.url, mode).await;
        let (_tx, mut cancel) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            native::attempt_at(&request("{}").unwrap(), &mut cancel, &broker.url),
        )
        .await
        .unwrap();
        assert_eq!(result, None);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
        assert!(fake.requests.lock().unwrap().is_empty());
        fake.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_missing_ack_id_recovers_by_correlation_without_guessing_caller() {
    let broker = test_broker::Broker::start();
    let fake = Fake::start(&broker.url, 8).await;
    let (_tx, mut cancel) = watch::channel(false);
    let result = tokio::time::timeout(Duration::from_secs(10),
        native::attempt_at(&request("{}").unwrap(), &mut cancel, &broker.url)).await.unwrap();
    assert_eq!(result, None);
    assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
    assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
    assert!(fake.requests.lock().unwrap().is_empty());
    fake.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_cancel_during_unacknowledged_call_is_bounded() {
    let broker = test_broker::Broker::start();
    let mut fake = Fake::start(&broker.url, 3).await;
    let (tx, mut cancel) = watch::channel(false);
    let url = broker.url.clone();
    let task = tokio::spawn(async move {
        native::attempt_at(&request("{}").unwrap(), &mut cancel, &url).await
    });
    tokio::time::timeout(Duration::from_secs(5), fake.started.recv())
        .await
        .unwrap()
        .unwrap();
    tx.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .unwrap()
            .unwrap(),
        None
    );
    assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
    fake.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_external_cancel_and_bad_accept_are_final() {
    for mode in [5, 6] {
        let broker = test_broker::Broker::start();
        let fake = Fake::start(&broker.url, mode).await;
        let (_tx, mut cancel) = watch::channel(false);
        let answer = tokio::time::timeout(
            Duration::from_secs(10),
            native::attempt_at(&request("{}").unwrap(), &mut cancel, &broker.url),
        )
        .await
        .unwrap();
        if mode == 5 {
            assert_eq!(answer, Some(Outcome::Cancelled));
        } else {
            assert!(matches!(answer, Some(Outcome::Failed(_))));
        }
        fake.stop().await;
    }
}
