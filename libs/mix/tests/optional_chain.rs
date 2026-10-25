// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `?.` optional field access: nil on a nil left side, a raise on any other
//! non-map, one step per `?.`, never an assignment target, never a method
//! call, and the ternary `?` and `??` keep their old meaning.

use mix::evaluator::Evaluator;
use mix::lexer::Lexer;
use mix::parser::Parser;

async fn run(src: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, src);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = mix::evaluator::SharedBuf::new();
    let stderr = mix::evaluator::SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

async fn out(src: &str) -> String {
    run(src)
        .await
        .unwrap_or_else(|e| panic!("{src:?} failed: {e}"))
        .trim_end()
        .to_string()
}

#[tokio::test]
async fn nil_left_side_reads_nil() {
    assert_eq!(out("$a = nil\nprint(type($a?.b))\n").await, "nil");
    assert_eq!(out("$a = {b: 1}\nprint($a?.b)\n").await, "1");
    assert_eq!(out("$a = {b: 1}\nprint(type($a?.zz))\n").await, "nil");
}

#[tokio::test]
async fn each_step_guards_only_itself() {
    // Guarded all the way: nil.
    assert_eq!(out("$a = {b: nil}\nprint(type($a?.b?.c))\n").await, "nil");
    // `.c` after a nil `?.b` is an ordinary access and raises, as before.
    let err = run("$a = {b: nil}\nprint($a?.b.c)\n")
        .await
        .expect_err("plain . on nil must still raise");
    assert!(err.contains("cannot access field 'c' on nil"), "got: {err}");
    // A deep read through a list: an out-of-range index is already nil.
    assert_eq!(
        out("$n = {commits: {nodes: []}}\nprint(type($n.commits?.nodes[0]?.commit))\n").await,
        "nil"
    );
    assert_eq!(
        out("$n = {commits: {nodes: [{commit: 7}]}}\nprint($n.commits?.nodes[0]?.commit)\n").await,
        "7"
    );
}

#[tokio::test]
async fn only_nil_is_forgiven() {
    for src in ["$s = \"text\"\nprint($s?.b)\n", "$l = [1]\nprint($l?.b)\n"] {
        let err = run(src).await.expect_err("a wrong shape must still raise");
        assert!(
            err.contains("cannot access field 'b'"),
            "{src:?} got: {err}"
        );
    }
}

#[tokio::test]
async fn combines_with_nil_coalesce() {
    assert_eq!(out("$a = nil\nprint($a?.b ?? \"dflt\")\n").await, "dflt");
}

#[tokio::test]
async fn method_calls_are_refused_at_parse_time() {
    let err = run("$a = nil\nprint($a?.len())\n")
        .await
        .expect_err("?.name( is not supported");
    assert!(err.contains("`?.len(` is not supported"), "got: {err}");
}

#[tokio::test]
async fn ternary_and_coalesce_are_unchanged() {
    assert_eq!(out("$c = true\nprint($c ? 1 : 2)\n").await, "1");
    // `? .x` with a space is not `?.`: still a parse error, as it always was.
    assert!(
        run("$c = true\n$m = {x: 1}\nprint($c ? .x : 2)\n")
            .await
            .is_err()
    );
    assert_eq!(out("$a = nil\nprint($a ?? 3)\n").await, "3");
}

#[tokio::test]
async fn more_receivers_and_positions() {
    // nil all the way down, guarded at every step.
    assert_eq!(out("$a = nil\nprint(type($a?.b?.c))\n").await, "nil");
    // A number is not forgiven either.
    let err = run("$n = 5\nprint($n?.b)\n")
        .await
        .expect_err("number receiver");
    assert!(err.contains("cannot access field 'b'"), "got: {err}");
    // Inside a ternary branch: `?` then `?.` side by side.
    assert_eq!(
        out("$c = true\n$a = nil\nprint(type($c ? $a?.b : 1))\n").await,
        "nil"
    );
    assert_eq!(
        out("$c = false\n$a = {b: 1}\nprint($c ? 0 : $a?.b)\n").await,
        "1"
    );
}

#[tokio::test]
async fn interpolation_and_heredocs_accept_question_dot() {
    // Interpolation paths already read nil from a nil step; `?.` means `.` there.
    assert_eq!(out("$a = {b: 7}\nprint(\"v=${a?.b}\")\n").await, "v=7");
    assert_eq!(
        out("$a = nil\nprint(\"v=${a?.b ?? \"dflt\"}\")\n").await,
        "v=dflt"
    );
    // The default must not swallow a value that exists.
    assert_eq!(
        out("$a = {b: 7}\nprint(\"v=${a?.b ?? \"dflt\"}\")\n").await,
        "v=7"
    );
    // Default text containing `?.` is left exactly as written.
    assert_eq!(
        out("$a = nil\nprint(\"${a?.b ?? \"what?.no\"}\")\n").await,
        "what?.no"
    );
    assert_eq!(
        out("$a = {b: {c: 3}}\n$s = <<EOF\nc=${a?.b?.c}\nEOF\nprint(trim($s))\n").await,
        "c=3"
    );
}

#[tokio::test]
async fn assignment_through_question_dot_is_a_parse_error() {
    for src in [
        "$a = {b: {}}\n$a?.b = 1\n",
        "$a = {b: {}}\n$a.b?.c = 1\n",
        "$a = {b: 1}\n$a?.b += 1\n",
    ] {
        let err = run(src)
            .await
            .expect_err("?. is never an assignment target");
        assert!(err.contains("Parse error"), "{src:?} got: {err}");
    }
}

#[test]
fn lint_reads_question_dot_like_dot() {
    use mix::analyzer::{AnalyzerConfig, analyze};
    let codes = |src: &str| -> Vec<String> {
        let tokens = Lexer::new(src).tokenize().expect("lexes");
        let stmts = Parser::new(tokens, src).parse_program().expect("parses");
        analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default())
            .diagnostics
            .into_iter()
            .map(|d| d.code.to_string())
            .collect()
    };
    // `null` after `?.` is a field name, exactly as after `.`: no MIX-W2312.
    assert!(
        !codes("$m = {}\nprint($m?.null)\n").contains(&"MIX-W2312".to_string()),
        "?.null flagged as the null bareword"
    );
    // The control: a bare `null` value still is.
    assert!(codes("print(null)\n").contains(&"MIX-W2312".to_string()));
    // A clean script using ?., ?? and a ternary lints with no errors.
    let src = "$a = nil\n$c = true\nprint($a?.b ?? 1)\nprint($c ? $a?.b : 2)\n";
    let errors: Vec<String> = codes(src)
        .into_iter()
        .filter(|c| c.starts_with("MIX-E"))
        .collect();
    assert!(errors.is_empty(), "lint errors: {errors:?}");
}

#[tokio::test]
async fn keyword_field_names_work_after_question_dot() {
    // Field names may lex as keywords after a dot ($m.to); the same after ?.
    assert_eq!(out("$m = {to: 5}\nprint($m?.to)\n").await, "5");
}
