// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A `fn` named after a builtin is refused at its definition (mix 0.118.0,
//! FN_SHADOWS_BUILTIN). The builtin wins at every call site, so such a
//! function could never be called by name; before the refusal a script ran
//! on and failed far from the cause (`fn run($argv)` died as "run():
//! argument 1 (cmd) must be string"), or quietly got the builtin's data.
//! The name set is `builtins::is_reserved_callable`, shared with lint's
//! MIX-E1303.

use mix::error::MixError;
use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;

/// Run `source`, returning its result and whatever it printed before any
/// failure (so a test can prove the refusal came before the first line).
async fn run(source: &str) -> (Result<(), MixError>, String) {
    let tokens = Lexer::new(source).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, source).parse_program().expect("parses");
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    let result = eval.execute(&stmts).await.map(|_| ());
    (result, stdout.to_string_lossy())
}

fn refused_code(result: &Result<(), MixError>) -> String {
    match result {
        Ok(()) => panic!("expected FN_SHADOWS_BUILTIN, the script ran"),
        Err(e) => e.info().map(|i| i.code.clone()).unwrap_or_else(|| e.to_string()),
    }
}

#[tokio::test]
async fn top_level_builtin_name_is_refused_before_the_first_statement() {
    let (result, out) =
        run("print(\"before\")\nfn run($a)\n  return run_argv($a)\nend\nprint(\"after\")\n").await;
    assert_eq!(refused_code(&result), "FN_SHADOWS_BUILTIN");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("fn run shadows the builtin run()"), "{msg}");
    assert!(msg.contains("line 2"), "the error points at the definition: {msg}");
    assert_eq!(out, "", "the hoisting pass refuses before anything runs");
}

#[tokio::test]
async fn higher_order_builtin_name_is_refused() {
    // count/map/filter are HOF builtins, outside is_builtin; lint missed
    // them before 0.118.0 too.
    let (result, _) = run("fn count($x)\n  return 1\nend\n").await;
    assert_eq!(refused_code(&result), "FN_SHADOWS_BUILTIN");
}

#[tokio::test]
async fn inline_special_form_name_is_refused() {
    let (result, _) = run("fn quit()\n  return 1\nend\n").await;
    assert_eq!(refused_code(&result), "FN_SHADOWS_BUILTIN");
}

#[tokio::test]
async fn eval_special_name_is_refused() {
    let (result, _) = run("fn printf($a) = \"USER\"\n").await;
    assert_eq!(refused_code(&result), "FN_SHADOWS_BUILTIN");
}

#[tokio::test]
async fn nested_definition_is_refused_when_reached() {
    // Not hoisted, so the lines before it run; the refusal comes when the
    // branch reaches the definition.
    let (result, out) =
        run("print(\"before\")\nif true then\n  fn filter($x)\n    return $x\n  end\nend\n").await;
    assert_eq!(refused_code(&result), "FN_SHADOWS_BUILTIN");
    assert_eq!(out, "before\n");
}

#[tokio::test]
async fn non_colliding_name_still_works() {
    let (result, out) = run("fn run_cmd($a)\n  return $a * 2\nend\nprint(run_cmd(21))\n").await;
    result.expect("a non-colliding name runs");
    assert_eq!(out, "42\n");
}

#[tokio::test]
async fn serve_extension_name_is_not_refused() {
    // `context` is a mix-shell extension, not a builtin: a plain evaluator
    // has no such extension, and a module's exported `context` stays
    // reachable as $m.context(). Lint warns (MIX-W2313); the runtime allows.
    let (result, out) = run("fn context()\n  return 7\nend\nprint(context())\n").await;
    result.expect("an extension name is not reserved");
    assert_eq!(out, "7\n");
}
