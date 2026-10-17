// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `mix_tokens()` — the real lexer's token stream as data (0.117.0).
//!
//! Tools that read Mix source used to hand-roll a line-state lexer (comments,
//! multi-line strings, interpolation, heredocs) and it drifted from the
//! grammar. These tests hold the builtin to the lexer itself: exact source
//! text, positions that point back into the source, and comments only when
//! asked for.

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::{Lexer, SourceToken};
use mix::parser::Parser;

async fn run(source: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, source);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

fn tok(kind: &'static str, text: &str, line: usize, column: usize) -> SourceToken {
    SourceToken {
        kind,
        text: text.into(),
        line,
        column,
    }
}

#[test]
fn kinds_text_and_positions_come_from_the_lexer() {
    let source = "$x = \"a ${b}\" -- c\nprint('q', 2 .. w)\n";
    assert_eq!(
        Lexer::source_tokens(source, true).unwrap(),
        vec![
            tok("variable", "$x", 1, 1),
            tok("operator", "=", 1, 4),
            tok("interp_string", "\"a ${b}\"", 1, 6),
            tok("comment", "-- c", 1, 15),
            tok("newline", "\n", 1, 19),
            tok("keyword", "print", 2, 1),
            tok("punct", "(", 2, 6),
            tok("string", "'q'", 2, 7),
            tok("punct", ",", 2, 10),
            tok("number", "2", 2, 12),
            tok("operator", "..", 2, 14),
            tok("word", "w", 2, 17),
            tok("punct", ")", 2, 18),
            tok("newline", "\n", 2, 19),
        ]
    );
    // Without the option the comment is simply absent, as the parser sees it.
    let plain = Lexer::source_tokens(source, false).unwrap();
    assert!(plain.iter().all(|t| t.kind != "comment"));
    assert_eq!(plain.len(), 13);
}

#[test]
fn comment_markers_inside_strings_and_heredocs_are_text() {
    let source =
        "$a = \"-- not\" # yes\n$b = 'multi\n# line'\n$c = <<EOF\n-- body # too\nEOF\n$d = 1\n";
    let tokens = Lexer::source_tokens(source, true).unwrap();
    let comments: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == "comment")
        .map(|t| (t.text.as_str(), t.line))
        .collect();
    assert_eq!(comments, vec![("# yes", 1)]);
    let multi = tokens
        .iter()
        .find(|t| t.text.starts_with("'multi"))
        .unwrap();
    assert_eq!((multi.kind, multi.line, multi.column), ("string", 2, 6));
    assert_eq!(multi.text, "'multi\n# line'");
    let heredoc = tokens.iter().find(|t| t.kind == "heredoc").unwrap();
    assert!(heredoc.text.starts_with("<<EOF") && heredoc.text.contains("-- body # too"));
    // Lines after a multi-line literal and a heredoc are still exact.
    let d = tokens.iter().find(|t| t.text == "$d").unwrap();
    assert_eq!((d.line, d.column), (7, 1));
}

#[test]
fn every_token_points_back_into_a_real_tool_source() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/verb_registry.mix");
    let source = std::fs::read_to_string(path).unwrap();
    let lines: Vec<Vec<char>> = source.split('\n').map(|l| l.chars().collect()).collect();
    let tokens = Lexer::source_tokens(&source, true).unwrap();
    assert!(tokens.len() > 500, "{}", tokens.len());
    for t in &tokens {
        let at: String = lines[t.line - 1][t.column - 1..].iter().collect();
        let first = t.text.split('\n').next().unwrap();
        assert!(
            at.starts_with(first),
            "{t:?} is not at {}:{}",
            t.line,
            t.column
        );
    }
    assert!(tokens.iter().any(|t| t.kind == "comment"));
}

#[tokio::test]
async fn the_builtin_returns_maps_and_validates_its_options() {
    let out = run(concat!(
        "$t = mix_tokens('$x = 1 -- c', {comments: true})\n",
        "print(json_encode($t))\n",
        "print(len(mix_tokens('$x = 1 -- c')))\n",
        "print(len(mix_tokens(\"\")))\n",
    ))
    .await
    .unwrap();
    assert_eq!(
        out,
        concat!(
            // json_encode sorts keys.
            "[{\"col\":1,\"kind\":\"variable\",\"line\":1,\"text\":\"$x\"},",
            "{\"col\":4,\"kind\":\"operator\",\"line\":1,\"text\":\"=\"},",
            "{\"col\":6,\"kind\":\"number\",\"line\":1,\"text\":\"1\"},",
            "{\"col\":8,\"kind\":\"comment\",\"line\":1,\"text\":\"-- c\"}]\n",
            "3\n0\n",
        )
    );
    for (call, code) in [
        ("mix_tokens(1)", "TYPE_MISMATCH"),
        ("mix_tokens(\"x\", {comment: true})", "OPTION_INVALID"),
        ("mix_tokens(\"x\", {comments: 1})", "OPTION_INVALID"),
        ("mix_tokens(\"x\", 1)", "TYPE_MISMATCH"),
    ] {
        let out = run(&format!(
            "try\n  $t = {call}\ncatch $msg, $err\n  print($err.code)\nend\n"
        ))
        .await
        .unwrap();
        assert_eq!(out, format!("{code}\n"), "{call}");
    }
}

#[tokio::test]
async fn source_that_does_not_lex_raises_with_its_position() {
    let out = run(concat!(
        "try\n",
        "  $t = mix_tokens('$ok = 1\n$s = \"never closed')\n",
        "catch $msg, $err\n",
        "  print($err.code .. \" \" .. to_string($err.details.line))\n",
        "end\n",
    ))
    .await
    .unwrap();
    assert_eq!(out, "MIX_SOURCE_LEX 2\n");
}
