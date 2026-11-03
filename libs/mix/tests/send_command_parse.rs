// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bus command slots share the narrow hyphenated target-name rule.

use mix::analyzer::{AnalyzerConfig, Severity, analyze};
use mix::ast::{BinOp, Expr, Stmt, StmtKind};
use mix::lexer::Lexer;
use mix::parser::Parser;

fn parse(source: &str) -> Vec<Stmt> {
    let tokens = Lexer::new(source).tokenize().expect("must lex");
    Parser::new(tokens, source)
        .parse_program()
        .unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn command(stmt: &Stmt) -> &Expr {
    match &stmt.kind {
        StmtKind::Send { command, .. } | StmtKind::Emit { command, .. } => command,
        StmtKind::Address { body, .. } => command(&body[0]),
        StmtKind::Assignment {
            value: Expr::Send { command, .. },
            ..
        } => command,
        other => panic!("unexpected statement {other:?}"),
    }
}

#[test]
fn all_command_slots_read_names_whole_and_lint_without_errors() {
    for name in [
        "waiter-lab.list",
        "next-hop.select.all",
        "end-lab.list",
        "send-lab.list",
        "svc-01.ping",
        "node-007.a.b",
        "a-1.2.3",
        "svc-x.01.002.ping",
        "_a-b_",
    ] {
        for source in [
            format!("send waiter-lab {name} timeout=1; print($rc)"),
            format!("emit waiter-lab {name}"),
            format!("address waiter-lab\n{name}\nend\nprint($rc)"),
            format!("$r = send waiter-lab {name}\nprint($r)"),
            format!("send (\"waiter-lab\") {name}; print($rc)"),
        ] {
            let program = parse(&source);
            assert!(
                matches!(command(&program[0]), Expr::StringLiteral(s) if s == name),
                "{source}"
            );
            let analysis = analyze(&program, None, &AnalyzerConfig::default());
            assert!(
                !analysis
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error),
                "{source}: {:?}",
                analysis.diagnostics
            );
        }
    }
}

#[test]
fn quoted_and_dynamic_commands_keep_their_meaning() {
    let program = parse("send t \"waiter-lab.list\"");
    assert!(matches!(command(&program[0]), Expr::StringLiteral(s) if s == "waiter-lab.list"));
    let program = parse("$cmd = \"waiter-lab.list\"; send t $cmd");
    assert!(matches!(command(&program[1]), Expr::Variable(s) if s == "cmd"));
    let program = parse("send waiter-lab (10 - 4)");
    assert!(matches!(
        command(&program[0]),
        Expr::BinaryOp { op: BinOp::Sub, .. }
    ));
    let program = parse("send waiter-lab (to_string(42))");
    assert!(matches!(command(&program[0]), Expr::FunctionCall { .. }));
}

#[test]
fn spaced_bare_subtraction_keeps_the_existing_command_and_argument_reading() {
    let program = parse("send t a - b");
    let StmtKind::Send { command, args, .. } = &program[0].kind else {
        panic!("send")
    };
    assert!(matches!(command, Expr::StringLiteral(s) if s == "a"));
    assert_eq!(args.len(), 1);
    assert!(matches!(&args[0].1, Expr::UnaryOp { .. }));
}

#[test]
fn malformed_segments_cannot_escape_into_expressions_or_lint_clean() {
    for source in [
        "$x = svc-01",
        "send t $cmd-007",
        "send t ping arg=svc-01",
        "send t svc-01()",
    ] {
        let result = Lexer::new(source)
            .tokenize()
            .and_then(|tokens| Parser::new(tokens, source).parse_program());
        assert!(
            result.is_err(),
            "{source} must be rejected before execution"
        );
    }
}

#[tokio::test]
async fn bare_commands_reach_the_bus_unchanged() {
    use mix::error::MixResult;
    use mix::evaluator::{BusHandler, Evaluator, IncomingEvent};
    use mix::value::Value;
    use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc};

    type BusFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);
    impl BusHandler for Recorder {
        fn send<'a>(
            &'a self,
            _: &'a str,
            command: &'a str,
            _: &'a Value,
        ) -> BusFuture<'a, MixResult<(i32, Value)>> {
            self.0.borrow_mut().push(command.into());
            Box::pin(async { Ok((0, Value::Nil)) })
        }
        fn emit<'a>(
            &'a self,
            _: &'a str,
            command: &'a str,
            _: &'a Value,
        ) -> BusFuture<'a, MixResult<()>> {
            self.0.borrow_mut().push(command.into());
            Box::pin(async { Ok(()) })
        }
        fn port_exists<'a>(&'a self, _: &'a str) -> BusFuture<'a, MixResult<bool>> {
            Box::pin(async { Ok(true) })
        }
        fn next_incoming<'a>(&'a self) -> BusFuture<'a, Option<IncomingEvent>> {
            Box::pin(async { None })
        }
    }
    let recorder = Rc::new(Recorder::default());
    let mut evaluator = Evaluator::new();
    evaluator.set_bus_handler(recorder.clone());
    let program = parse(
        "send waiter-lab waiter-lab.list\nemit waiter-lab svc-01.changed\naddress waiter-lab\nnext-hop.1.2.3\nend\n$r = send waiter-lab node-007.ping",
    );
    evaluator.execute(&program).await.expect("must execute");
    assert_eq!(
        *recorder.0.borrow(),
        [
            "waiter-lab.list",
            "svc-01.changed",
            "next-hop.1.2.3",
            "node-007.ping"
        ]
    );
}
