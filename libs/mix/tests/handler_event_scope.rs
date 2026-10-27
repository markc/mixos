// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `$event` dynamic extent + handler-fault observability (0.63.0).
//!
//! The historical failure shape (hub 2133f4b): a fn called from an `on`
//! handler raised `NAME_UNDEFINED` on `$event`, serve-mode fault
//! isolation swallowed the raise, and the citizen stayed registered and
//! subscribed with noded reporting delivered=1 while its side effects
//! silently never happened — a blind-but-healthy citizen, found after an
//! hour of live bisection. Two fixes, both pinned here: the event is
//! bound with dynamic extent so handler-called fns see it, and an
//! isolated fault now reaches the stderr sink + the serve runtime's
//! health hook. The load-bearing test is raise-then-survive-then-
//! OBSERVABLE — a survive-only test would have passed before the fix.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mix::error::{MixError, MixResult};
use mix::evaluator::{
    BusFuture, BusHandler, Evaluator, IncomingEvent, REPLY_SHUTTING_DOWN_CODE, ReservedOutcome,
    ServeRuntime, SharedBuf,
};
use mix::lexer::Lexer;
use mix::parser::Parser;
use mix::value::Value;

fn mk_event(command: &str, body: &str, headers: &[(&str, &str)]) -> IncomingEvent {
    let mut h = BTreeMap::new();
    for (k, v) in headers {
        h.insert(k.to_string(), v.to_string());
    }
    IncomingEvent {
        generation: 0,
        command: command.to_string(),
        headers: h,
        body: body.to_string(),
    }
}

struct Setup {
    eval: Evaluator,
    stdout: SharedBuf,
    stderr: SharedBuf,
}

async fn setup(source: &str) -> Setup {
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    let mut lexer = Lexer::new(source);
    let stmts = Parser::new(lexer.tokenize().unwrap(), source)
        .parse_program()
        .unwrap();
    eval.execute(&stmts).await.unwrap();
    Setup {
        eval,
        stdout,
        stderr,
    }
}

// ---------- $event dynamic extent ----------

#[tokio::test]
async fn rc_result_reply_are_isolated_per_handler_invocation() {
    // 2026-09-25 entry: $rc/$result/$reply used to ride the shared global,
    // so an interleaved handler overwrote them. They are now saved per
    // invocation and restored after (like $event) — the handler reads its
    // OWN $rc, and the outer $rc survives.
    let source = r#"
$rc = 42
$observed = ""
on first.evt
  $rc = 7
  $observed = $rc
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("first.evt", "", &[]))
        .await
        .unwrap();
    // The handler read its own value during execution...
    assert_eq!(s.eval.get_global("observed").unwrap().to_mix_string(), "7");
    // ...and the outer value was restored afterwards, not clobbered.
    assert_eq!(s.eval.get_global("rc").unwrap().to_mix_string(), "42");
}

#[tokio::test]
async fn fn_called_from_handler_sees_event() {
    // The exact pre-fix failure: record() reads $event and used to raise
    // NAME_UNDEFINED (function frames fall through to globals only).
    let source = r#"
$recorded = ""
fn describe()
  return $event.command .. "|" .. $event.headers["topic"] .. "|" .. $event.body
end
on play.note
  $recorded = describe()
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("play.note", "C4", &[("topic", "music.notes")]))
        .await
        .unwrap();
    assert_eq!(
        s.eval.get_global("recorded").unwrap().to_mix_string(),
        "play.note|music.notes|C4"
    );
    assert!(
        s.stderr.to_string_lossy().is_empty(),
        "no fault: {}",
        s.stderr.to_string_lossy()
    );
}

#[tokio::test]
async fn event_seen_two_calls_deep() {
    let source = r#"
$got = ""
fn inner()
  return $event.body
end
fn outer()
  return inner()
end
on deep.msg
  $got = outer()
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("deep.msg", "payload", &[]))
        .await
        .unwrap();
    assert_eq!(s.eval.get_global("got").unwrap().to_mix_string(), "payload");
}

#[tokio::test]
async fn fn_local_event_parameter_still_shadows() {
    // Normal resolution order: a fn's own $event binding wins over the
    // dispatch-scoped one.
    let source = r#"
$got = ""
fn with_own($event)
  return $event
end
on shadow.msg
  $got = with_own("mine")
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("shadow.msg", "dispatch-body", &[]))
        .await
        .unwrap();
    assert_eq!(s.eval.get_global("got").unwrap().to_mix_string(), "mine");
}

#[tokio::test]
async fn preexisting_global_event_is_restored_after_dispatch() {
    // The dynamic binding is dispatch-scoped: a top-level $event global
    // (legal outside handlers) survives the dispatch unchanged.
    let source = r#"
$event = "preexisting"
on restore.msg
  $ignore = 1
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("restore.msg", "x", &[]))
        .await
        .unwrap();
    assert_eq!(
        s.eval.get_global("event").unwrap().to_mix_string(),
        "preexisting"
    );
}

// ---------- fault observability: raise, survive, OBSERVABLE ----------

#[tokio::test]
async fn handler_lambdas_capture_locals_and_keep_globals_live() {
    let source = r#"
$recorded = nil
$escaped = nil
$global_value = "before"
on closure.msg
  $names = map([{name: "alpha"}, {name: "beta"}], fn($s) = $s.name)
  $recorded = all(["alpha", "beta"], fn($s) = contains($names, $s))
  $escaped = fn($unused) = map($names, fn($name) = $name .. $global_value)
end
"#;
    let mut s = setup(source).await;
    s.eval
        .dispatch_event(mk_event("closure.msg", "", &[]))
        .await
        .unwrap();
    assert_eq!(
        s.eval.get_global("recorded").unwrap().to_mix_string(),
        "true"
    );
    let source = r#"
$global_value = "after"
print($escaped(nil))
"#;
    let mut lexer = Lexer::new(source);
    let stmts = Parser::new(lexer.tokenize().unwrap(), source)
        .parse_program()
        .unwrap();
    s.eval.execute(&stmts).await.unwrap();
    assert_eq!(s.stdout.to_string_lossy(), "[alphaafter, betaafter]\n");
    assert!(
        s.stderr.to_string_lossy().is_empty(),
        "{}",
        s.stderr.to_string_lossy()
    );
}

/// Test double capturing the health hook.
struct RecordingRuntime {
    faults: RefCell<Vec<String>>,
}

impl ServeRuntime for RecordingRuntime {
    fn handle_reserved(
        &self,
        _command: &str,
        _args_header: Option<&str>,
        _req_body: &str,
        _handler_commands: &[(&str, Option<&str>)],
        _correlated: bool,
    ) -> Option<ReservedOutcome> {
        None
    }
    fn record_handler_fault(&self, summary: &str) {
        self.faults.borrow_mut().push(summary.to_string());
    }
}

#[tokio::test]
async fn handler_fault_survives_and_is_observable() {
    let source = r#"
$after = ""
fn blow_up()
  die("deliberate fault")
end
on play.note
  blow_up()
end
on play.note
  $after = "second handler still ran"
end
"#;
    let mut s = setup(source).await;
    let runtime = Rc::new(RecordingRuntime {
        faults: RefCell::new(Vec::new()),
    });
    s.eval.set_serve_runtime(runtime.clone());

    s.eval
        .dispatch_event(mk_event("play.note", "C4", &[]))
        .await
        .unwrap();

    // Survive: the citizen keeps its handlers and later handlers ran.
    assert_eq!(s.eval.handler_count(), 2);
    assert_eq!(
        s.eval.get_global("after").unwrap().to_mix_string(),
        "second handler still ran"
    );
    // Observable half 1: the stderr sink (tracing may be routed nowhere).
    let err = s.stderr.to_string_lossy();
    assert!(
        err.contains("handler fault: play.note[0]"),
        "fault must reach stderr: {err}"
    );
    assert!(err.contains("deliberate fault"), "{err}");
    assert!(err.contains("handler remains registered"), "{err}");
    // Observable half 2: the health hook fired exactly once.
    {
        let faults = runtime.faults.borrow();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(faults[0].contains("play.note[0]"), "{faults:?}");
    }

    // And the citizen still dispatches afterwards.
    s.eval
        .dispatch_event(mk_event("play.note", "D4", &[]))
        .await
        .unwrap();
    assert_eq!(runtime.faults.borrow().len(), 2);
    assert!(s.stdout.to_string_lossy().is_empty());
}

#[tokio::test]
async fn healthy_dispatch_records_nothing() {
    let source = "on ok.msg\n  $x = 1\nend\n";
    let mut s = setup(source).await;
    let runtime = Rc::new(RecordingRuntime {
        faults: RefCell::new(Vec::new()),
    });
    s.eval.set_serve_runtime(runtime.clone());
    s.eval
        .dispatch_event(mk_event("ok.msg", "x", &[]))
        .await
        .unwrap();
    assert!(runtime.faults.borrow().is_empty());
    assert!(s.stderr.to_string_lossy().is_empty());
}

/// Count evaluator error records without installing a process-global logger.
#[derive(Clone, Default)]
struct ErrorRecords(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for ErrorRecords {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct RefusedReply {
    shutdown: bool,
    terminal: RefCell<Vec<(u8, String)>>,
}

impl BusHandler for RefusedReply {
    fn emit<'a>(&'a self, _: &'a str, _: &'a str, _: &'a Value) -> BusFuture<'a, MixResult<()>> {
        Box::pin(async { unreachable!("test only replies") })
    }

    fn port_exists<'a>(&'a self, _: &'a str) -> BusFuture<'a, MixResult<bool>> {
        Box::pin(async { unreachable!("test only replies") })
    }

    fn next_incoming<'a>(&'a self) -> BusFuture<'a, Option<IncomingEvent>> {
        Box::pin(async { unreachable!("test dispatches directly") })
    }

    fn send<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> BusFuture<'a, MixResult<(i32, Value)>> {
        Box::pin(async { unreachable!("test only replies") })
    }

    fn reply<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Option<&'a str>,
        _: u8,
        _: &'a str,
    ) -> BusFuture<'a, MixResult<()>> {
        Box::pin(async move {
            // Identical prose: only the transport marker changes classification.
            let msg = "reply failed: supervised client is shutting down";
            Err(if self.shutdown {
                MixError::structured(REPLY_SHUTTING_DOWN_CODE, msg)
            } else {
                MixError::RuntimeError {
                    msg: msg.into(),
                    span: None,
                }
            })
        })
    }

    fn reply_shutdown_synth<'a>(
        &'a self,
        to: &'a str,
        command: &'a str,
        id: Option<&'a str>,
        rc: u8,
        body: &'a str,
    ) -> BusFuture<'a, MixResult<()>> {
        Box::pin(async move {
            assert_eq!((to, command, id), ("caller", "refresh", Some("request-1")));
            self.terminal.borrow_mut().push((rc, body.into()));
            Ok(())
        })
    }
}

#[tokio::test]
async fn shutdown_refusal_stays_drain_visible_without_a_handler_fault() {
    // Class S completes before the drain; Class C completes DURING its grace.
    // Catching the refusal must also preserve the pending caller's handle.
    for source in [
        "fn answer()\n reply(0, \"ok\")\nend\non refresh\n answer()\nend\n",
        "on refresh async\n reply(0, \"ok\")\nend\n",
        "on refresh async\n try\n reply(0, \"ok\")\n catch $err\n end\nend\n",
        // No explicit reply: the normal NO_REPLY synthesis hits the same gate.
        "on refresh async\n $x = 1\nend\n",
    ] {
        tokio::task::LocalSet::new()
            .run_until(async {
                let records = ErrorRecords::default();
                let _guard = tracing::subscriber::set_default(records.clone());
                let mut s = setup(source).await;
                let runtime = Rc::new(RecordingRuntime {
                    faults: RefCell::new(Vec::new()),
                });
                let bus = Rc::new(RefusedReply {
                    shutdown: true,
                    terminal: RefCell::new(Vec::new()),
                });
                s.eval.set_serve_runtime(runtime.clone());
                s.eval.set_bus_handler(bus.clone());
                s.eval
                    .dispatch_event(mk_event(
                        "refresh",
                        "",
                        &[("type", "request"), ("from", "caller"), ("id", "request-1")],
                    ))
                    .await
                    .unwrap();
                let outcome = s
                    .eval
                    .drain_class_c_for_shutdown(std::time::Duration::from_secs(1), true)
                    .await;
                assert_eq!(outcome.synth_sent, 1, "{source}: {outcome:?}");
                assert_eq!(outcome.synth_failed, 0);
                assert_eq!(outcome.aborted, 0);
                let terminal = bus.terminal.borrow();
                assert_eq!(terminal.len(), 1);
                assert_eq!(terminal[0].0, 16);
                assert!(terminal[0].1.contains("HANDLER_CANCELLED"));
                assert_eq!(records.0.load(std::sync::atomic::Ordering::Relaxed), 0);
                assert!(runtime.faults.borrow().is_empty());
                assert!(s.stderr.to_string_lossy().is_empty());
                assert_eq!(s.eval.handler_count(), 1);
            })
            .await;
    }
}

#[tokio::test]
async fn a_real_fault_after_a_caught_shutdown_refusal_is_still_a_fault() {
    // The refusal sets the handle's shutdown flag; only the refusal itself may
    // be quiet. An unrelated raise afterwards must stay an observable fault.
    tokio::task::LocalSet::new()
        .run_until(async {
            let records = ErrorRecords::default();
            let _guard = tracing::subscriber::set_default(records.clone());
            let mut s = setup(
                "on refresh async\n try\n reply(0, \"ok\")\n catch $err\n end\n raise(\"OTHER\", \"real bug\")\nend\n",
            )
            .await;
            let runtime = Rc::new(RecordingRuntime {
                faults: RefCell::new(Vec::new()),
            });
            let bus = Rc::new(RefusedReply {
                shutdown: true,
                terminal: RefCell::new(Vec::new()),
            });
            s.eval.set_serve_runtime(runtime.clone());
            s.eval.set_bus_handler(bus.clone());
            s.eval
                .dispatch_event(mk_event(
                    "refresh",
                    "",
                    &[("type", "request"), ("from", "caller"), ("id", "request-1")],
                ))
                .await
                .unwrap();
            let outcome = s
                .eval
                .drain_class_c_for_shutdown(std::time::Duration::from_secs(1), true)
                .await;
            assert_eq!(runtime.faults.borrow().len(), 1, "the raise is a fault");
            assert!(records.0.load(std::sync::atomic::Ordering::Relaxed) >= 1);
            assert!(s.stderr.to_string_lossy().contains("OTHER"));
            assert_eq!(outcome.synth_sent, 1, "{outcome:?}");
            let terminal = bus.terminal.borrow();
            assert_eq!(terminal.len(), 1, "exactly one terminal reply");
            // 16 today; 15 (HANDLER_FAULT) once the drain labels faulted handles.
            assert!(matches!(terminal[0].0, 15 | 16), "{:?}", terminal[0]);
        })
        .await;
}

#[tokio::test]
async fn other_reply_failure_remains_an_observable_handler_fault() {
    let records = ErrorRecords::default();
    let _guard = tracing::subscriber::set_default(records.clone());
    let mut s = setup("on refresh\n reply(0, \"ok\")\nend\n").await;
    let runtime = Rc::new(RecordingRuntime {
        faults: RefCell::new(Vec::new()),
    });
    s.eval.set_serve_runtime(runtime.clone());
    s.eval.set_bus_handler(Rc::new(RefusedReply {
        shutdown: false,
        terminal: RefCell::new(Vec::new()),
    }));
    s.eval
        .dispatch_event(mk_event(
            "refresh",
            "",
            &[("type", "request"), ("from", "caller"), ("id", "request-1")],
        ))
        .await
        .unwrap();
    assert_eq!(runtime.faults.borrow().len(), 1);
    assert!(
        s.stderr
            .to_string_lossy()
            .contains("handler fault: refresh[0]")
    );
    assert!(
        s.stderr
            .to_string_lossy()
            .contains("handler remains registered")
    );
    // Handler error and the failed HANDLER_FAULT synthesis both stay errors.
    assert_eq!(records.0.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(s.eval.handler_count(), 1);
}

#[tokio::test]
async fn shutdown_code_raised_by_script_still_records_a_fault() {
    let records = ErrorRecords::default();
    let _guard = tracing::subscriber::set_default(records.clone());
    let source = "on refresh\n raise(\"BUS_REPLY_SHUTTING_DOWN\", \"script fault\")\nend\n";
    let mut s = setup(source).await;
    let runtime = Rc::new(RecordingRuntime {
        faults: RefCell::new(Vec::new()),
    });
    s.eval.set_serve_runtime(runtime.clone());
    s.eval
        .dispatch_event(mk_event("refresh", "", &[]))
        .await
        .unwrap();
    assert_eq!(runtime.faults.borrow().len(), 1);
    assert!(s.stderr.to_string_lossy().contains("script fault"));
    assert_eq!(records.0.load(std::sync::atomic::Ordering::Relaxed), 1);
}
