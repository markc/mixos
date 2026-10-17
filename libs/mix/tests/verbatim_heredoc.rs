// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `<<'TAG'` — the verbatim heredoc (TODO-mix 2026-10-09).
//!
//! `<<'EOF'` was a lexer error, so any text holding `${` had to be escaped
//! by hand, and agents reached for zsh heredocs or side files instead. The
//! quoted tag takes the body byte for byte, `mix lint` treats it as opaque
//! text (no MIX-W2402), and an `ssh_mix` body written this way is linted as
//! remote Mix source like any other literal.

use mix::analyzer::{AnalyzerConfig, analyze};
use mix::lexer::Lexer;
use mix::parser::Parser;
use mix::value::Value;

async fn eval(source: &str) -> Value {
    mix::run(source).await.expect("run")
}

fn diags(source: &str) -> Vec<(String, Option<usize>, String)> {
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let stmts = Parser::new(tokens, source).parse_program().expect("parse");
    let cfg = AnalyzerConfig {
        source: Some(source.to_string()),
        ..AnalyzerConfig::default()
    };
    analyze(&stmts, None, &cfg)
        .diagnostics
        .into_iter()
        .map(|d| (d.code.to_string(), d.line, d.message))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn the_body_round_trips_byte_exact() {
    // Everything the plain heredoc would touch, plus a line that merely
    // contains the tag.
    let body = "${x} \\${y} $5 ~/x it's \"q\" back\\slash \\n $(not run)\n\
                not EOF yet\n  EOF and more\n\ttab";
    let src = format!("$s = <<'EOF'\n{body}\nEOF\n$s\n");
    assert_eq!(eval(&src).await, Value::String(body.to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn it_ends_only_at_the_tag_line_and_keeps_empty_lines() {
    let src = "$s = <<'END'\n\nfirst\n\nEND\n$s\n";
    assert_eq!(eval(src).await, Value::String("\nfirst\n".to_string()));
    let empty = "$s = <<'END'\nEND\n$s\n";
    assert_eq!(eval(empty).await, Value::String(String::new()));
}

#[test]
fn an_unterminated_or_malformed_quoted_tag_is_a_lexer_error() {
    let err = Lexer::new("$s = <<'EOF\nx\nEOF\n").tokenize().unwrap_err();
    assert!(format!("{err}").contains("<<'TAG'"), "{err}");
    let err = Lexer::new("$s = <<'EOF'\nno closer\n")
        .tokenize()
        .unwrap_err();
    assert!(format!("{err}").contains("unterminated heredoc"), "{err}");
}

#[test]
fn lint_reports_no_w2402_inside_a_verbatim_heredoc() {
    // The plain heredoc warns about a bare `$name` that is a bound name;
    // the verbatim one is opaque text.
    let plain = "$name = 1\n$s = <<EOF\nhello $name\nEOF\nprint($s)\n";
    assert!(
        diags(plain).iter().any(|(c, ..)| c == "MIX-W2402"),
        "control: the plain heredoc must still warn"
    );
    let verbatim = "$name = 1\n$s = <<'EOF'\nhello $name ${name}\nEOF\nprint($s)\n";
    let found = diags(verbatim);
    assert!(
        !found.iter().any(|(c, ..)| c == "MIX-W2402"),
        "verbatim heredoc must be opaque to W2402: {found:?}"
    );
}

#[test]
fn an_ssh_mix_verbatim_body_is_linted_as_remote_source() {
    // Line 3 calls an undefined function inside the body: lint reports it
    // at its real file line, and there is no "not analysed" MIX-D3012.
    let src =
        "$r = ssh_mix(\"host\", <<'EOF'\nprint(\"${remote_side}\")\nno_such_fn_zz()\nEOF\n)\n";
    let found = diags(src);
    assert!(
        !found.iter().any(|(c, ..)| c == "MIX-D3012"),
        "a verbatim body is a literal: {found:?}"
    );
    let hit = found
        .iter()
        .find(|(c, _, m)| c == "MIX-E1102" && m.contains("inside ssh_mix body"))
        .unwrap_or_else(|| panic!("the body was not analysed: {found:?}"));
    assert_eq!(hit.1, Some(3), "mapped to the real line: {found:?}");
}
