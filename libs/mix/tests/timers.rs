// SPDX-License-Identifier: MIT OR Apache-2.0
#![cfg(target_os = "linux")]
use mix::{
    evaluator::{Evaluator, ReservedOutcome, ServeRuntime},
    lexer::Lexer,
    parser::Parser,
    value::Value,
};
use std::{rc::Rc, time::Duration};

async fn exec(e: &mut Evaluator, source: &str) -> mix::MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    e.execute(&Parser::new(tokens, source).parse_program()?)
        .await
}
async fn pump(e: &mut Evaluator) {
    tokio::time::timeout(Duration::from_secs(5), e.run_event_pump())
        .await
        .unwrap()
        .unwrap();
}
struct Runtime;
impl ServeRuntime for Runtime {
    fn handle_reserved(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: &[(&str, Option<&str>)],
        _: bool,
    ) -> Option<ReservedOutcome> {
        None
    }
}

#[tokio::test(flavor = "current_thread")]
async fn plain_after_and_cancelled_interval_exit_the_pump() {
    let mut e = Evaluator::new();
    assert_eq!(
        exec(&mut e, "after(\"a=b\", \"=\")").await.unwrap(),
        Value::String("b".into())
    );
    exec(
        &mut e,
        r#"
$ticks = 0
$a = after(0.01, "once")
$i = interval(0.1, "repeat")
on timer.tick
    $ticks = $ticks + 1
    if $event.args.tag == "repeat" then cancel_timer($i) end
end
"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    assert_eq!(e.get_global("ticks"), Some(Value::Number(2.0)));
    assert_eq!(
        exec(&mut e, "cancel_timer($a)").await.unwrap(),
        Value::Bool(false)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serve_citizen_counts_ticks() {
    let mut e = Evaluator::new();
    e.set_serve_runtime(Rc::new(Runtime));
    exec(
        &mut e,
        r#"
$ticks = 0
$id = interval(0.1, "sample")
on timer.tick
    $ticks = $ticks + 1
    if $ticks == 3 then
        cancel_timer($id)
        quit()
    end
end
"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    assert_eq!(e.get_global("ticks"), Some(Value::Number(3.0)));
}

#[tokio::test(flavor = "current_thread")]
async fn candidate_and_old_generation_retire_independently() {
    let mut old = Evaluator::new();
    let mut candidate = Evaluator::new();
    candidate.set_reload_candidate(true);
    exec(&mut old, "$id = interval(0.1, \"old\")")
        .await
        .unwrap();
    exec(&mut candidate, "$id = after(0.01, \"candidate\")")
        .await
        .unwrap();
    candidate.close_native_events();
    assert_eq!(
        exec(&mut old, "cancel_timer($id)").await.unwrap(),
        Value::Bool(true)
    );
    let mut replacement = Evaluator::new();
    exec(&mut replacement, "$id = after(0.01, \"replacement\")")
        .await
        .unwrap();
    old.close_native_events();
    assert!(exec(&mut old, "after(1, \"closed\")").await.is_err());
    pump(&mut replacement).await;
    assert_eq!(
        exec(&mut replacement, "cancel_timer($id)").await.unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn timer_lints() {
    let source = "interval(0.01, \"fast\")\non timer.tick\nread_file(\"/tmp/settings\")\nstat(\"/proc/loadavg\")\nexists(\"/sys/class\")\nend";
    let stmts = Parser::new(Lexer::new(source).tokenize().unwrap(), source)
        .parse_program()
        .unwrap();
    let a = mix::analyzer::analyze(&stmts, None, &Default::default());
    assert_eq!(
        a.diagnostics
            .iter()
            .filter(|d| d.code == "MIX-E1513")
            .count(),
        1
    );
    assert_eq!(
        a.diagnostics
            .iter()
            .filter(|d| d.code == "MIX-W2410")
            .count(),
        1
    );
}

#[test]
fn expression_mode_preserves_text_after_and_refuses_numeric_after() {
    use mix::evaluator::{EvalLimits, eval_expr_string};
    assert_eq!(
        eval_expr_string("after(\"a=b\", \"=\")", &[], None, EvalLimits::default()).unwrap(),
        Value::String("b".into())
    );
    assert!(eval_expr_string("after(1, \"tick\")", &[], None, EvalLimits::default()).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn busy_pump_coalesces_and_cancel_discards_due_tick() {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
$id = interval(0.1, "busy")
$ticks = 0
$missed = 0
on timer.tick
    $ticks = $ticks + 1
    $missed = $event.args.missed
    cancel_timer($id)
end
"#,
    )
    .await
    .unwrap();
    std::thread::sleep(Duration::from_millis(360));
    pump(&mut e).await;
    assert_eq!(e.get_global("ticks"), Some(Value::Number(1.0)));
    assert!(e.get_global("missed").unwrap().to_number().unwrap() >= 2.0);
    exec(&mut e, "$cancelled = after(0.001, \"discard\")")
        .await
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(
        exec(&mut e, "cancel_timer($cancelled)").await.unwrap(),
        Value::Bool(true)
    );
    pump(&mut e).await;
    assert_eq!(e.get_global("ticks"), Some(Value::Number(1.0)));
    let err = exec(&mut e, "interval(0.099, \"floor\")")
        .await
        .unwrap_err();
    assert_eq!(err.info().unwrap().code, "TIMER_PERIOD");
}
