// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A member call (`$m.name(...)`) on a map holding a function member calls
//! that member in every mode, `--serve` included. Under serve mode an
//! extension used to win: a citizen that required a module exporting
//! `fn context` got the mix shell's `context()` string back from
//! `$r.context({})` (TODO-mix 2026-10-10). With no such member the call is
//! still UFCS, so the extension stays reachable.

use mix::{
    MixResult,
    evaluator::{Evaluator, ReservedOutcome, ServeRuntime},
    lexer::Lexer,
    parser::Parser,
    value::Value,
};
use std::rc::Rc;

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

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

/// An evaluator with the shell's `context` extension stand-in registered.
fn evaluator(serve: bool) -> Evaluator {
    let mut eval = Evaluator::new();
    eval.register(
        "context",
        mix::sync_ext(|_| Ok(Value::String("extension".to_string()))),
    );
    if serve {
        eval.set_serve_runtime(Rc::new(Runtime));
    }
    eval
}

/// A module exporting `fn context`, written to a fresh directory.
fn module() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mix-member-ext-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t").replace("::", "-")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ctxmod.mix");
    std::fs::write(&path, "fn context($over)\n  return {state_dir: \"/srv\"}\nend\n").unwrap();
    path
}

#[tokio::test(flavor = "current_thread")]
async fn a_module_member_beats_an_extension_in_serve_mode() {
    let path = module();
    for serve in [true, false] {
        let mut eval = evaluator(serve);
        let src = format!(
            "$r = require({:?})\n$got = $r.context({{}}).state_dir\n",
            path.display().to_string()
        );
        exec(&mut eval, &src).await.unwrap();
        assert_eq!(
            eval.get_global("got").map(|v| v.to_mix_string()),
            Some("/srv".to_string()),
            "serve={serve}: the module's own function must run"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_literal_map_member_beats_an_extension_in_serve_mode() {
    let mut eval = evaluator(true);
    exec(
        &mut eval,
        "$m = {context: fn($x) return \"mine\" end}\n$got = $m.context(1)\n",
    )
    .await
    .unwrap();
    assert_eq!(
        eval.get_global("got").map(|v| v.to_mix_string()),
        Some("mine".to_string())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn with_no_member_the_extension_is_still_reached_by_ufcs() {
    for serve in [true, false] {
        let mut eval = evaluator(serve);
        exec(&mut eval, "$s = \"x\"\n$got = $s.context()\n")
            .await
            .unwrap();
        assert_eq!(
            eval.get_global("got").map(|v| v.to_mix_string()),
            Some("extension".to_string()),
            "serve={serve}"
        );
    }
}
