// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parser pin: an `on` header reads a hyphenated name whole.
//!
//! Before 0.113.1 `on dopus-test.command.executed` stopped at the hyphen:
//! the handler registered as `dopus` and `-test.command.executed` became
//! its body, so it parsed, linted clean and never fired. The header now
//! uses the same scan as a hyphenated `send` target.

use mix::ast::{Stmt, StmtKind};
use mix::lexer::Lexer;
use mix::parser::Parser;

fn parse(source: &str) -> Vec<Stmt> {
    let tokens = Lexer::new(source).tokenize().expect("source must lex");
    Parser::new(tokens, source)
        .parse_program()
        .unwrap_or_else(|e| panic!("{source:?} must parse: {e:?}"))
}

/// The handler's name and body length.
fn on_parts(stmt: &Stmt) -> (&str, usize) {
    let StmtKind::On { command, body, .. } = &stmt.kind else {
        panic!("expected On, got {:?}", stmt.kind);
    };
    (command, body.len())
}

#[test]
fn hyphenated_names_are_read_whole() {
    for name in [
        "dopus-test.command.executed",
        "dopus-hl-4242.command.executed",
        "comp-nested.surface.mapped",
        "a-b",
        "svc-01.ping",
        "node-007.a.b",
        "next-hop.select.all",
    ] {
        let src = format!("on {name}\n  reply(\"ok\")\nend");
        let prog = parse(&src);
        assert_eq!(prog.len(), 1, "{src}");
        assert_eq!(on_parts(&prog[0]), (name, 1), "{src}");
    }
}

#[test]
fn bare_and_quoted_spellings_agree() {
    let bare = parse("on dopus-test.command.executed\n  print(1)\nend");
    let quoted = parse("on \"dopus-test.command.executed\"\n  print(1)\nend");
    assert_eq!(on_parts(&bare[0]), on_parts(&quoted[0]));
}

#[test]
fn trailer_and_one_line_body_still_parse() {
    let prog = parse("on dopus-test.x desc \"Doc\" async\n  reply(1)\nend");
    let StmtKind::On {
        command,
        doc,
        is_async,
        ..
    } = &prog[0].kind
    else {
        panic!("expected On");
    };
    assert_eq!(command, "dopus-test.x");
    assert_eq!(doc.as_deref(), Some("Doc"));
    assert!(*is_async);

    let prog = parse("on a-b.c; print(1); end");
    assert_eq!(on_parts(&prog[0]), ("a-b.c", 1));

    // A comment after the name is not part of it.
    let prog = parse("on a-b.c -- note\n  print(1)\nend");
    assert_eq!(on_parts(&prog[0]), ("a-b.c", 1));
}

/// Names without a hyphen take the old path unchanged.
#[test]
fn plain_names_are_unchanged() {
    for name in [
        "dopus.command.executed",
        "topic.delivery",
        "ced.select",
        "x",
    ] {
        let src = format!("on {name}\n  reply(1)\nend");
        assert_eq!(on_parts(&parse(&src)[0]), (name, 1), "{src}");
    }
}
