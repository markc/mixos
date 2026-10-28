// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use serde_json::json;
mod native_tests;
#[cfg(all(target_os = "linux", feature = "portal"))]
mod portal_tests;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn frame(ctx: &Context, viewport: ViewportId, handle: &mut Handle) -> Option<Outcome> {
    let mut input = egui::RawInput { viewport_id: viewport, ..Default::default() };
    input.viewports.insert(viewport, Default::default());
    let mut outcome = None;
    ctx.run_ui(input, |ui| outcome = handle.poll(ui.ctx())).drop_without_applying_deltas();
    outcome
}

#[test]
fn secondary_viewport_receives_and_wakes_only_itself() {
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let child = ViewportId::from_hash_of("compose-3");
    ctx.run_ui(Default::default(), |ui| {
        ui.ctx().show_viewport_deferred(child, Default::default(), |_, _| {});
    }).drop_without_applying_deltas();
    let (wake_tx, wake_rx) = mpsc::channel();
    ctx.set_repaint_observer(move |info| {
        if std::thread::current().name() == Some("toolkit-choose") { let _ = wake_tx.send(info.viewport_id); }
    });
    let mut handle = choose(request(r#"{"correlation":"compose-3"}"#).unwrap());
    let (release, wait) = mpsc::channel();
    handle.driver = Some(Box::new(move |req, _, emitter| {
        assert_eq!(req.correlation.as_deref(), Some("compose-3"));
        wait.recv().unwrap();
        emitter.send(Message::Answer(Outcome::Cancelled));
    }));
    assert!(frame(&ctx, child, &mut handle).is_none());
    assert_eq!(handle.viewport(), Some(child));
    release.send(()).unwrap();
    assert_eq!(wake_rx.recv_timeout(Duration::from_secs(5)).unwrap(), child);
    assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
    assert_eq!(frame(&ctx, child, &mut handle), Some(Outcome::Cancelled));
    assert!(frame(&ctx, child, &mut handle).is_none());
}

#[test]
fn cascade_waits_for_each_end_cancel_stops_and_failure_advances() {
    for (bus, system, expected, system_calls) in [
        (Some(Outcome::Cancelled), None, Tier::Bus, 0),
        (None, Some(Outcome::Cancelled), Tier::System, 1),
        (Some(Outcome::Failed("BUSY".into())), Some(Outcome::Failed("Response 2".into())), Tier::InProcess, 1),
        (None, None, Tier::InProcess, 1),
    ] {
        let ctx = Context::default();
        let (tx, rx) = mpsc::channel();
        let emitter = Emitter {tx, ctx, viewport: ViewportId::ROOT};
        let (_cancel_tx, cancel) = watch::channel(false);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        runtime().block_on(cascade(async {bus}, async { calls.fetch_add(1, Ordering::SeqCst); system }, cancel, &emitter));
        let messages: Vec<_> = rx.try_iter().collect();
        let last = messages.last().unwrap();
        let actual = if matches!(last, Message::Fallback) { Tier::InProcess } else {
            messages.iter().filter_map(|m| if let Message::Tier(t) = m {Some(*t)} else {None}).last().unwrap()
        };
        assert_eq!(actual, expected);
        assert_eq!(calls.load(Ordering::SeqCst), system_calls);
    }
}

#[test]
fn cancel_is_final_and_every_failure_falls_through() {
    assert!(final_outcome(&Some(Outcome::Cancelled)));
    assert!(!final_outcome(&Some(Outcome::Failed("failure".into()))));
    assert!(!final_outcome(&None));
}

#[test]
fn chosen_preserves_paths_filter_and_choice_defaults() {
    let request = request(&json!({"multiple":true,"filters":[{"name":"Text","patterns":["*.md"]}],
        "choices":[{"id":"encoding","label":"Encoding","options":[{"id":"utf8","label":"UTF-8"}]},
        {"id":"readonly","label":"Read only","options":[]}]}).to_string()).unwrap();
    let outcome = decode_outcome(&request, &json!({"response":0,"uris":["file:///tmp/%FF.md","file:///tmp/two.md"]}));
    let Outcome::Chosen {paths, choices, current_filter} = outcome else { panic!("not chosen") };
    assert_eq!(path_bytes(&paths[0]), b"/tmp/\xff.md");
    assert_eq!(choices["encoding"], "utf8");
    assert_eq!(choices["readonly"], "false");
    assert_eq!(current_filter, request.current_filter);
    assert!(final_outcome(&Some(Outcome::Chosen {paths, choices, current_filter})));
}

#[test]
fn bus_outcome_refuses_shapes_not_offered() {
    let request = request("{}").unwrap();
    assert!(matches!(decode_outcome(&request, &json!({"response":0,"uris":[]})), Outcome::Failed(_)));
    assert!(matches!(decode_outcome(&request, &json!({"response":1})), Outcome::Cancelled));
    assert!(matches!(decode_outcome(&request, &json!({"response":2})), Outcome::Failed(_)));
}
