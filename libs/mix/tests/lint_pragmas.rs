// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `-- lint:` pragmas (TODO-mix 2026-10-09): in-file declarations and
//! suppressions, so `mix lint _bin/*.mix` can come out clean without a
//! per-invocation `--allow-global` that cannot live with the file.

use mix::analyzer::{AnalyzerConfig, Diagnostic, analyze};
use mix::lexer::Lexer;
use mix::parser::Parser;

fn diags(src: &str) -> Vec<Diagnostic> {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, src).parse_program().expect("parses");
    let cfg = AnalyzerConfig {
        source: Some(src.to_string()),
        ..AnalyzerConfig::default()
    };
    analyze(&stmts, Some("test.mix"), &cfg).diagnostics
}

fn codes(src: &str) -> Vec<(String, Option<usize>)> {
    diags(src)
        .into_iter()
        .map(|d| (d.code.to_string(), d.line))
        .collect()
}

fn has(src: &str, code: &str) -> bool {
    codes(src).iter().any(|(c, _)| c == code)
}

/// A heredoc that reports MIX-W2402 (a bare bound `$pid` is literal text).
const HEREDOC: &str = "$s = <<EOF\nkill $pid\nEOF\n";

#[test]
fn globals_declares_an_injected_name() {
    // The t6_fixture_snapshot.mix shape: a remote body whose caller binds
    // $sha through ssh_mix bindings.
    let body = "if trim(run_argv_must([\"git\", \"rev-parse\", \"HEAD\"])) != $sha then\n  raise(\"REVISION\", \"moved\")\nend\nprint(slice($sha, 0, 12))\n";
    assert!(has(body, "MIX-E1101"), "precondition: {:?}", codes(body));
    let declared = format!("-- lint: globals sha\n{body}");
    assert_eq!(codes(&declared), vec![], "one pragma clears every read");
    // `$` is tolerated, and several names may be listed.
    let src = "-- lint: globals $a, b c\nprint($a .. $b .. $c)\n";
    assert_eq!(codes(src), vec![]);
}

#[test]
fn ignore_suppresses_its_code_on_the_next_statement_only() {
    let src = format!("$pid = 1\n-- lint: ignore MIX-W2402\n{HEREDOC}{HEREDOC}print($s)\n");
    let found = codes(&src);
    let w2402: Vec<_> = found.iter().filter(|(c, _)| c == "MIX-W2402").collect();
    assert_eq!(
        w2402,
        vec![&("MIX-W2402".to_string(), Some(6))],
        "only the second heredoc still reports: {found:?}"
    );
    assert!(!found.iter().any(|(c, _)| c == "MIX-D3019"), "{found:?}");
}

#[test]
fn ignore_is_code_specific() {
    // The next statement reports W2402; an ignore for another code leaves
    // it, and is itself stale.
    let src = format!("$pid = 1\n-- lint: ignore MIX-E1101\n{HEREDOC}print($s)\n");
    let found = codes(&src);
    assert!(
        found.contains(&("MIX-W2402".to_string(), Some(3))),
        "{found:?}"
    );
    assert!(
        found.contains(&("MIX-D3019".to_string(), Some(2))),
        "{found:?}"
    );
}

#[test]
fn ignore_written_after_code_covers_its_own_line() {
    let src = "print($nope) -- lint: ignore MIX-E1101\nprint($also_nope)\n";
    let found = codes(src);
    assert_eq!(found, vec![("MIX-E1101".to_string(), Some(2))], "{found:?}");
}

#[test]
fn ignore_file_covers_the_whole_file() {
    let src = format!("-- lint: ignore-file MIX-W2402\n$pid = 1\n{HEREDOC}{HEREDOC}print($s)\n");
    assert_eq!(codes(&src), vec![]);
}

#[test]
fn codes_are_matched_case_insensitively_and_without_the_prefix() {
    let src = format!("$pid = 1\n-- lint: ignore w2402\n{HEREDOC}print($s)\n");
    assert_eq!(codes(&src), vec![]);
    let src = format!("$pid = 1\n# lint: ignore mix-w2402\n{HEREDOC}print($s)\n");
    assert_eq!(codes(&src), vec![]);
}

#[test]
fn a_bare_ignore_is_refused() {
    for pragma in [
        "-- lint: ignore",
        "-- lint: ignore-file",
        "-- lint: ignore ,",
    ] {
        let src = format!("$pid = 1\n{pragma}\n{HEREDOC}print($s)\n");
        let found = codes(&src);
        assert!(
            found.contains(&("MIX-E1601".to_string(), Some(2))),
            "{pragma}: {found:?}"
        );
        // A refused pragma suppresses nothing.
        assert!(found.iter().any(|(c, _)| c == "MIX-W2402"), "{found:?}");
    }
}

#[test]
fn malformed_pragmas_are_errors() {
    for pragma in [
        "-- lint: ignore MIX-W9999",
        "-- lint: ignore not-a-code",
        "-- lint: globals",
        "-- lint: globals 1abc",
        "-- lint: frobnicate",
        "-- lint:",
    ] {
        let found = codes(&format!("{pragma}\nprint(1)\n"));
        assert!(
            found.contains(&("MIX-E1601".to_string(), Some(1))),
            "{pragma}: {found:?}"
        );
    }
    let d = diags("-- lint: ignore\nprint(1)\n");
    let e = d.iter().find(|d| d.code == "MIX-E1601").unwrap();
    assert!(e.message.contains("bare"), "{}", e.message);
    assert!(e.hint.as_deref().unwrap().contains("MIX-W2402"));
}

#[test]
fn a_stale_suppression_is_a_note_at_the_pragma() {
    let found = codes("print(1)\n-- lint: ignore MIX-W2402\nprint(2)\n");
    assert_eq!(found, vec![("MIX-D3019".to_string(), Some(2))]);
    let found = codes("-- lint: ignore-file MIX-W2402\nprint(1)\n");
    assert_eq!(found, vec![("MIX-D3019".to_string(), Some(1))]);
    // Nothing follows the pragma at all.
    let found = codes("print(1)\n-- lint: ignore MIX-W2402\n");
    assert_eq!(found, vec![("MIX-D3019".to_string(), Some(2))]);
    let d = diags("print(1)\n-- lint: ignore MIX-W2402\n");
    assert_eq!(d[0].severity, mix::analyzer::Severity::Note);
}

#[test]
fn a_pragma_inside_a_string_or_heredoc_is_not_read() {
    // Generated Mix source written out by a script carries its own pragmas;
    // they belong to the generated file, not to this one.
    let src = "$gen = <<EOF\n-- lint: globals zz\nEOF\nprint($gen .. $zz)\n";
    assert!(has(src, "MIX-E1101"), "{:?}", codes(src));
    let src = "$gen = \"line one\n-- lint: ignore-file MIX-E1101\n\"\nprint($gen .. $zz)\n";
    assert!(has(src, "MIX-E1101"), "{:?}", codes(src));
}

#[test]
fn an_ignore_covers_every_statement_on_the_next_line() {
    // The target is a line (eslint's disable-next-line), so several
    // statements on it are all covered, and no refusal is needed.
    let src = "-- lint: ignore MIX-E1101\nprint($first); print($second)\nprint($third)\n";
    assert_eq!(codes(src), vec![("MIX-E1101".to_string(), Some(3))]);
    let src = "-- lint: ignore MIX-E1101\nif true then print($missing) end\n";
    assert_eq!(codes(src), vec![]);
    let src = "-- lint: ignore MIX-E1101\n$v = if true then print($missing) end\nprint($v)\n";
    assert_eq!(codes(src), vec![]);
    // Written after the code, it covers its own line.
    let src = "print($first); print($second) -- lint: ignore MIX-E1101\n";
    assert_eq!(codes(src), vec![]);
}

#[test]
fn an_ignore_skips_blank_and_comment_lines_to_the_next_code_line() {
    let src = "$pid = 1\n-- lint: ignore MIX-W2402\n\n-- the generated restart script\n$s = <<EOF\nkill $pid\nEOF\nprint($s)\n";
    assert_eq!(codes(src), vec![]);
}

#[test]
fn an_ignore_inside_a_fn_expression_body_targets_its_own_line() {
    let src = "fn f() = if true then\n  -- lint: ignore MIX-W2312\n  print(null)\nend\nprint(null)\nprint(f())\n";
    assert_eq!(codes(src), vec![("MIX-W2312".to_string(), Some(5))]);
}

#[test]
fn an_ignore_covers_one_line_of_a_multi_line_statement() {
    // A diagnostic on the third line of a statement is not on the line after
    // the pragma; the pragma goes on the line before the finding instead.
    let src = "-- lint: ignore MIX-W2312\n$m = {\n  a: 1,\n  b: null\n}\nprint($m)\n";
    assert_eq!(
        codes(src),
        vec![
            ("MIX-W2312".to_string(), Some(4)),
            ("MIX-D3019".to_string(), Some(1)),
        ]
    );
    let src = "$m = {\n  a: 1,\n  -- lint: ignore MIX-W2312\n  b: null\n}\nprint($m)\n";
    assert_eq!(codes(src), vec![]);
    // Nested terminators and branch lines do not matter: only the next line.
    let src = "-- lint: ignore MIX-W2312\n$f = fn() return 1\nend == null\nprint($f)\n";
    assert_eq!(
        codes(src),
        vec![
            ("MIX-W2312".to_string(), Some(3)),
            ("MIX-D3019".to_string(), Some(1)),
        ]
    );
}

#[test]
fn an_ssh_mix_body_honours_its_own_pragmas() {
    let src =
        "$r = ssh_mix(\"h\", <<EOF\n-- lint: ignore MIX-W2312\nprint(null)\nEOF\n)\nprint($r.ok)\n";
    assert_eq!(codes(src), vec![]);
    let src =
        "$r = ssh_mix(\"h\", <<EOF\n-- lint: globals sha\nprint($sha)\nEOF\n)\nprint($r.ok)\n";
    assert_eq!(codes(src), vec![]);
    // A stale one is reported, mapped into this file.
    let src =
        "$r = ssh_mix(\"h\", <<EOF\n-- lint: ignore MIX-W2312\nprint(1)\nEOF\n)\nprint($r.ok)\n";
    let found = codes(src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "MIX-D3019");
}

#[test]
fn an_ignore_does_not_reach_a_following_elif_line() {
    let src = "$x = 1\nif $x == 1 then\n  -- lint: ignore MIX-W2312\n  print(null)\nelif null then\n  print(2)\nend\n";
    assert_eq!(codes(src), vec![("MIX-W2312".to_string(), Some(5))]);
    let src = "print(if true then\n  -- lint: ignore MIX-W2312\n  print(1)\nelif null then\n  print(2)\nend)\n";
    let found = codes(src);
    assert!(
        found.contains(&("MIX-W2312".to_string(), Some(4))),
        "{found:?}"
    );
    assert!(
        found.contains(&("MIX-D3019".to_string(), Some(2))),
        "{found:?}"
    );
}

#[test]
fn words_after_a_pipe_are_shell_text_not_pragmas() {
    // sol round 6: this prints `-- lint: ignore`; it is not a bare ignore.
    assert_eq!(codes("print(1) | echo -- lint: ignore\n"), vec![]);
    // Nor can such text suppress anything.
    let src = "print(1) | echo -- lint: ignore-file MIX-E1101\nprint($nope)\n";
    assert_eq!(codes(src), vec![("MIX-E1101".to_string(), Some(2))]);
}

#[test]
fn a_block_header_pragma_covers_only_the_header() {
    let src = "-- lint: ignore MIX-E1101\nif $nope then\n  print($also)\nend\n";
    let found = codes(src);
    assert_eq!(found, vec![("MIX-E1101".to_string(), Some(3))], "{found:?}");
}

#[test]
fn pragmas_need_the_source_text() {
    // An embedder that does not hand over the source gets no pragmas, and
    // no pragma errors either.
    let src = "-- lint: globals sha\nprint($sha)\n";
    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let found = analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default()).diagnostics;
    assert!(found.iter().any(|d| d.code == "MIX-E1101"));
}
