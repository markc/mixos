// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `let i = 0` and a bareword `i = …` were already parse errors, but ones that
//! named the token after the habit (`expected end of statement, got
//! String("i")`). The parser now names the fix (TODO-mix 2026-10-17).

use mix::lexer::Lexer;
use mix::parser::Parser;

fn parse_error(src: &str) -> String {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    match Parser::new(tokens, src).parse_program() {
        Ok(_) => panic!("{src:?} parsed"),
        Err(e) => e.to_string(),
    }
}

fn parses(src: &str) -> bool {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    Parser::new(tokens, src).parse_program().is_ok()
}

#[test]
fn a_declaration_keyword_names_the_mix_form() {
    for kw in ["let", "var", "const"] {
        let msg = parse_error(&format!("{kw} i = 0\n"));
        assert!(msg.contains(&format!("Mix has no `{kw}`")), "{msg}");
        assert!(msg.contains("`$i = …`"), "{msg}");
        assert!(msg.contains("line 1"), "{msg}");
    }
    let msg = parse_error("let $i = 0\n");
    assert!(msg.contains("drop it and write `$i = …`"), "{msg}");
}

#[test]
fn a_bareword_assignment_names_the_mix_form_on_its_own_line() {
    // The 2026-10-17 shape: the `while` is already Mix; line 4 is not.
    let msg = parse_error("$i = 0\nwhile $i < 2\n  print($i)\n  i = $i + 1\nend\n");
    assert!(msg.contains("bareword assignment `i = …`"), "{msg}");
    assert!(msg.contains("`$i = …`"), "{msg}");
    assert!(msg.contains("line 4"), "{msg}");
}

#[test]
fn quoted_heads_and_valid_programs_are_untouched() {
    // A quoted "let" is a string, so the old message stands.
    let msg = parse_error("\"let\" = 1\n");
    assert!(!msg.contains("Mix has no"), "{msg}");
    assert!(!msg.contains("bareword assignment"), "{msg}");
    for src in [
        "let\n",
        "print(\"let i = 0\")\n",
        "$let = 1\nprint($let)\n",
        "$m = {let: 1, var: 2}\nprint($m)\n",
        "send svc restart force=1\nprint($rc)\n",
    ] {
        assert!(parses(src), "{src:?} must still parse");
    }
}
