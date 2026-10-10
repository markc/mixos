// SPDX-License-Identifier: MIT OR Apache-2.0
//! `on @.verb`: the serve-name placeholder.
//!
//! Parser half: `@` (unquoted) is accepted only as the whole first segment
//! of an `on` name, the AST keeps the static `@.verb` form, and every other
//! placement (`on a.@.b`, `on @`, `on @x.y`, quoted `on "@.x"`, an
//! expression) is a parse error that names the one valid form.
//!
//! Evaluator half: registration replaces `@` with the serve runtime's
//! service name, so the handler map, dispatch and the HELP listing all see
//! `<svc>.verb`; outside serve mode the statement raises the catchable
//! `SERVE_PREFIX_OUTSIDE_SERVE`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use mix::ast::{Stmt, StmtKind};
use mix::error::{MixError, MixResult};
use mix::evaluator::{
    BusHandler, Evaluator, IncomingEvent, ReservedOutcome, ServeRuntime, SharedBuf,
};
use mix::lexer::Lexer;
use mix::parser::Parser;
use mix::value::Value;
use tokio::sync::mpsc;

fn parse(source: &str) -> Vec<Stmt> {
    let tokens = Lexer::new(source).tokenize().expect("source must lex");
    Parser::new(tokens, source)
        .parse_program()
        .unwrap_or_else(|e| panic!("{source:?} must parse: {e:?}"))
}

/// The parse error's text; panics if the source lexes AND parses.
fn parse_err(source: &str) -> String {
    let tokens = match Lexer::new(source).tokenize() {
        Ok(t) => t,
        Err(e) => panic!("{source:?} must lex (a parse error is expected, not a lex error): {e}"),
    };
    match Parser::new(tokens, source).parse_program() {
        Ok(_) => panic!("{source:?} must not parse"),
        Err(e) => e.to_string(),
    }
}

fn on_command(stmts: &[Stmt]) -> (&str, Option<&str>, bool) {
    let StmtKind::On {
        command,
        doc,
        is_async,
        ..
    } = &stmts[0].kind
    else {
        panic!("expected an on statement, got {:?}", stmts[0].kind);
    };
    (command, doc.as_deref(), *is_async)
}

// ── parser: accepted forms ─────────────────────────────────────────────

#[test]
fn placeholder_name_parses_to_the_static_form() {
    let p = parse("on @.call\n  print(1)\nend\n");
    assert_eq!(on_command(&p), ("@.call", None, false));
}

#[test]
fn placeholder_takes_several_dotted_segments() {
    let p = parse("on @.a.b.c\nend\n");
    assert_eq!(on_command(&p).0, "@.a.b.c");
}

#[test]
fn placeholder_segments_may_be_keywords_like_any_dotted_name() {
    let p = parse("on @.select\nend\n");
    assert_eq!(on_command(&p).0, "@.select");
    let p = parse("on @.x.if.end\nend\n");
    assert_eq!(on_command(&p).0, "@.x.if.end");
}

#[test]
fn desc_and_async_trailers_work_unchanged() {
    let p = parse("on @.call desc \"Forward a call\" async\nend\n");
    assert_eq!(on_command(&p), ("@.call", Some("Forward a call"), true));
    let p = parse("on @.call async desc \"Forward a call\"\nend\n");
    assert_eq!(on_command(&p), ("@.call", Some("Forward a call"), true));
}

#[test]
fn literal_names_are_unchanged() {
    let p = parse("on photo.call\nend\n");
    assert_eq!(on_command(&p).0, "photo.call");
    let p = parse("on \"photo.call\"\nend\n");
    assert_eq!(on_command(&p).0, "photo.call");
}

// ── parser: refused forms ──────────────────────────────────────────────

#[test]
fn bare_at_without_a_verb_is_refused() {
    let e = parse_err("on @\nend\n");
    assert!(e.contains("must be followed by `.<verb>`"), "{e}");
}

#[test]
fn at_glued_to_a_word_is_refused() {
    let e = parse_err("on @x.y\nend\n");
    assert!(e.contains("must be followed by `.<verb>`"), "{e}");
}

#[test]
fn at_dot_with_no_segment_is_refused() {
    let e = parse_err("on @.\nend\n");
    assert!(e.contains("expected a verb name after `@.`"), "{e}");
}

#[test]
fn at_in_a_later_segment_is_refused() {
    let e = parse_err("on a.@.b\nend\n");
    assert!(e.contains("only valid as the whole first segment"), "{e}");
    let e = parse_err("on @.a.@.b\nend\n");
    assert!(e.contains("only valid as the whole first segment"), "{e}");
}

#[test]
fn quoted_at_name_is_refused() {
    let e = parse_err("on \"@.call\"\nend\n");
    assert!(e.contains("a quoted name cannot start with `@`"), "{e}");
    let e = parse_err("on \"@\"\nend\n");
    assert!(e.contains("a quoted name cannot start with `@`"), "{e}");
}

#[test]
fn quoted_segments_after_the_placeholder_must_be_bare_names() {
    for (source, seg) in [
        ("on @.\"@.call\"\nend\n", "\"@.call\""),
        ("on @.a.\"@\".b\nend\n", "\"@\""),
        ("on @.\"\"\nend\n", "\"\""),
        ("on @.\"a.b\"\nend\n", "\"a.b\""),
        ("on @.\" x\"\nend\n", "\" x\""),
    ] {
        let e = parse_err(source);
        assert!(
            e.contains(&format!("on: {seg} is not a valid segment after `@.`")),
            "{source:?}: {e}"
        );
    }
    // A quoted segment that IS a bare name is the same as writing it bare.
    let p = parse("on @.\"call\"\nend\n");
    assert_eq!(on_command(&p).0, "@.call");
}

#[test]
fn at_in_an_expression_is_a_parse_error_naming_the_one_use() {
    let e = parse_err("$x = @\n");
    assert!(e.contains("serve-name placeholder"), "{e}");
}

#[test]
fn at_inside_a_string_stays_literal() {
    let p = parse("$x = \"user@host\"\n");
    assert_eq!(p.len(), 1);
}

// ── evaluator ──────────────────────────────────────────────────────────

struct ChannelHandler {
    rx: RefCell<Option<mpsc::UnboundedReceiver<IncomingEvent>>>,
}

impl BusHandler for ChannelHandler {
    fn send<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> Pin<Box<dyn Future<Output = MixResult<(i32, Value)>> + 'a>> {
        Box::pin(async move { Ok((0, Value::Nil)) })
    }
    fn emit<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> Pin<Box<dyn Future<Output = MixResult<()>> + 'a>> {
        Box::pin(async move { Ok(()) })
    }
    fn port_exists<'a>(
        &'a self,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = MixResult<bool>> + 'a>> {
        Box::pin(async move { Ok(false) })
    }
    fn next_incoming<'a>(&'a self) -> Pin<Box<dyn Future<Output = Option<IncomingEvent>> + 'a>> {
        Box::pin(async move {
            let mut rx = self.rx.borrow_mut().take()?;
            let result = rx.recv().await;
            *self.rx.borrow_mut() = Some(rx);
            result
        })
    }
}

/// A serve runtime with a fixed service name that records the handler
/// listing HELP receives.
struct NamedRuntime {
    name: &'static str,
    help: RefCell<Vec<(String, Option<String>)>>,
}

impl ServeRuntime for NamedRuntime {
    fn handle_reserved(
        &self,
        command: &str,
        _args_header: Option<&str>,
        _req_body: &str,
        handler_commands: &[(&str, Option<&str>)],
        _correlated: bool,
    ) -> Option<ReservedOutcome> {
        if command != "HELP" {
            return None;
        }
        *self.help.borrow_mut() = handler_commands
            .iter()
            .map(|(c, d)| (c.to_string(), d.map(str::to_string)))
            .collect();
        Some(ReservedOutcome {
            rc: 0,
            body: "[]".to_string(),
            quit: false,
            reload: false,
        })
    }

    fn service_name(&self) -> Option<&str> {
        Some(self.name)
    }
}

fn event(command: &str, body: &str) -> IncomingEvent {
    IncomingEvent {
        generation: 0,
        command: command.to_string(),
        headers: BTreeMap::new(),
        body: body.to_string(),
    }
}

async fn run_plain(source: &str) -> Result<String, MixError> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await?;
    Ok(stdout.to_string_lossy())
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_registers_dispatches_and_lists_under_the_serve_name() {
    let source = r#"
$trace = ""
on @.call desc "Forward a call"
  $trace = $trace .. "call:" .. $event.body .. ";"
end
on @.call
  $trace = $trace .. "second:" .. $event.body .. ";"
end
on @.ping
  $trace = $trace .. "ping;"
end
on literal.verb
  $trace = $trace .. "literal;"
end
"#;
    let mut eval = Evaluator::new();
    let (tx, rx) = mpsc::unbounded_channel::<IncomingEvent>();
    eval.set_bus_handler(Rc::new(ChannelHandler {
        rx: RefCell::new(Some(rx)),
    }));
    let runtime = Rc::new(NamedRuntime {
        name: "photo",
        help: RefCell::new(Vec::new()),
    });
    eval.set_serve_runtime(runtime.clone());
    eval.execute(&parse(source)).await.unwrap();

    // Registered under the resolved name; two `on @.call` append, exactly
    // like two literal `on photo.call`.
    assert_eq!(eval.handler_command_count(), 3);
    assert!(eval.handler_is_async("photo.call", 0).is_some());
    assert!(eval.handler_is_async("photo.call", 1).is_some());
    assert!(eval.handler_is_async("photo.ping", 0).is_some());
    assert!(eval.handler_is_async("@.call", 0).is_none());

    tx.send(event("HELP", "")).unwrap();
    tx.send(event("photo.call", "A")).unwrap();
    tx.send(event("@.call", "never")).unwrap();
    tx.send(event("photo.ping", "")).unwrap();
    tx.send(event("literal.verb", "")).unwrap();
    drop(tx);
    eval.run_event_pump().await.unwrap();

    assert_eq!(
        eval.get_global("trace").unwrap().to_mix_string(),
        "call:A;second:A;ping;literal;",
        "the resolved name dispatches; the placeholder form is not a verb"
    );
    let mut names: Vec<String> = runtime
        .help
        .borrow()
        .iter()
        .map(|(c, _)| c.clone())
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names, ["literal.verb", "photo.call", "photo.ping"]);
    assert!(
        runtime
            .help
            .borrow()
            .iter()
            .any(|(c, d)| c == "photo.call" && d.as_deref() == Some("Forward a call")),
        "HELP carries the doc under the resolved name: {:?}",
        runtime.help.borrow()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_instance_resolves_to_its_own_name() {
    for name in ["photo", "other"] {
        let mut eval = Evaluator::new();
        eval.set_serve_runtime(Rc::new(NamedRuntime {
            name,
            help: RefCell::new(Vec::new()),
        }));
        eval.execute(&parse("on @.call\nend\n")).await.unwrap();
        assert!(eval.handler_is_async(&format!("{name}.call"), 0).is_some());
        assert_eq!(eval.handler_command_count(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn outside_serve_mode_the_placeholder_raises_a_coded_error() {
    let err = run_plain("on @.call\nend\n").await.unwrap_err();
    let info = err.info().expect("a structured error");
    assert_eq!(info.code, "SERVE_PREFIX_OUTSIDE_SERVE");
    assert!(info.message.contains("mix --serve"), "{}", info.message);
}

#[tokio::test(flavor = "current_thread")]
async fn outside_serve_mode_the_error_is_catchable() {
    let out = run_plain(
        "try\n  on @.call\n  end\ncatch $m, $e\n  print($e.code)\nend\nprint(\"after\")\n",
    )
    .await
    .unwrap();
    assert_eq!(out, "SERVE_PREFIX_OUTSIDE_SERVE\nafter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn literal_names_still_register_outside_serve_mode() {
    run_plain("on photo.call\nend\n").await.unwrap();
}
