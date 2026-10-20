// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Member calls into `require()`d modules (MIX-W2314 / MIX-W2315) and
//! functions that collide with a mix shell extension (MIX-W2313).
//! TODO-mix 2026-10-09 and 2026-10-10.

use mix::analyzer::{AnalyzerConfig, Diagnostic, MIX_SHELL_EXTENSIONS, analyze};
use mix::lexer::Lexer;
use mix::parser::Parser;
use std::path::{Path, PathBuf};

/// A fresh directory holding `files` (path relative to it → source).
fn tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mix-lint-req-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (name, src) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
    }
    dir
}

fn lint_at(caller: &Path, src: &str, cfg: &AnalyzerConfig) -> Vec<Diagnostic> {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, src).parse_program().expect("parses");
    analyze(&stmts, Some(caller.to_str().unwrap()), cfg).diagnostics
}

fn codes_at(dir: &Path, src: &str) -> Vec<(String, Option<usize>)> {
    lint_at(&dir.join("main.mix"), src, &AnalyzerConfig::default())
        .into_iter()
        .map(|d| (d.code.to_string(), d.line))
        .collect()
}

const LIB1: &str = "fn helper($a, $b)\n  return $a\nend\nfn opt($a, $b = 2)\n  return $a\nend\nfn _private()\n  return 1\nend\n$version = 1\n";

#[test]
fn the_probe_reports_arity_and_a_missing_export() {
    let dir = tree("probe", &[("lib1.mix", LIB1)]);
    let found = codes_at(
        &dir,
        "$m = require(\"./lib1.mix\")\nprint($m.helper(1))\nprint($m.nosuch(2))\n",
    );
    assert_eq!(
        found,
        vec![
            ("MIX-W2314".to_string(), Some(2)),
            ("MIX-W2315".to_string(), Some(3)),
        ]
    );
    let d = lint_at(
        &dir.join("main.mix"),
        "$m = require(\"./lib1.mix\")\nprint($m.helpr(1, 2))\n",
        &AnalyzerConfig::default(),
    );
    assert_eq!(d[0].code, "MIX-W2315");
    assert!(d[0].message.contains("./lib1.mix"), "{}", d[0].message);
    let hint = d[0].hint.as_deref().unwrap();
    assert!(hint.contains("did you mean 'helper'"), "{hint}");
    assert!(hint.contains("version"), "{hint}");
}

#[test]
fn correct_calls_defaults_and_exported_vars_stay_clean() {
    let dir = tree("clean", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nprint($m.helper(1, 2))\nprint($m.opt(1))\nprint($m.opt(1, 2))\nprint($m.version)\nprint($m.version(1))\n";
    assert_eq!(codes_at(&dir, src), vec![]);
    let found = codes_at(
        &dir,
        "$m = require(\"./lib1.mix\")\nprint($m.opt(1, 2, 3))\n",
    );
    assert_eq!(found, vec![("MIX-W2314".to_string(), Some(2))]);
}

#[test]
fn a_private_name_is_not_an_export() {
    let dir = tree("private", &[("lib1.mix", LIB1)]);
    let d = lint_at(
        &dir.join("main.mix"),
        "$m = require(\"./lib1.mix\")\nprint($m._private())\n",
        &AnalyzerConfig::default(),
    );
    assert_eq!(d[0].code, "MIX-W2315");
    assert!(d[0].hint.as_deref().unwrap().contains("private"));
}

#[test]
fn unproven_bindings_stand_down() {
    let dir = tree("standdown", &[("lib1.mix", LIB1)]);
    for src in [
        // A dynamic path.
        "$p = \"./lib1.mix\"\n$m = require($p)\nprint($m.nosuch())\n",
        // A rebound $m.
        "$m = require(\"./lib1.mix\")\n$m = {nosuch: fn() return 1 end}\nprint($m.nosuch())\n",
        // A field write onto $m.
        "$m = require(\"./lib1.mix\")\n$m.nosuch = fn() return 1 end\nprint($m.nosuch())\n",
        // $m is also a parameter somewhere.
        "$m = require(\"./lib1.mix\")\nfn f($m)\n  return $m.nosuch()\nend\nprint(f({}))\n",
        // A source/include can rebind anything.
        "$m = require(\"./lib1.mix\")\ninclude \"./lib1.mix\"\nprint($m.nosuch())\n",
    ] {
        let found = codes_at(&dir, src);
        assert!(
            !found
                .iter()
                .any(|(c, _)| c == "MIX-W2315" || c == "MIX-W2314"),
            "{src:?}: {found:?}"
        );
    }
}

#[test]
fn a_module_whose_return_replaces_its_exports_stands_down() {
    let dir = tree(
        "return",
        &[(
            "ret.mix",
            "fn helper($a, $b)\n  return $a\nend\nreturn {go: helper}\n",
        )],
    );
    let found = codes_at(
        &dir,
        "$m = require(\"./ret.mix\")\nprint($m.go(1))\nprint($m.helper(1))\n",
    );
    assert_eq!(found, vec![]);
}

#[test]
fn other_callables_answer_a_missing_member() {
    let dir = tree("ufcs", &[("lib1.mix", LIB1)]);
    // A function this file defines (UFCS), a prelude or shell extension,
    // and an address block (an unknown member call is a send there).
    let src = "$m = require(\"./lib1.mix\")\nfn extra($x)\n  return 1\nend\nprint($m.extra())\naddress svc\n  $m.restart()\nend\n";
    assert_eq!(codes_at(&dir, src), vec![]);
    let cfg = AnalyzerConfig {
        extensions: MIX_SHELL_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        ..AnalyzerConfig::default()
    };
    let found = lint_at(
        &dir.join("main.mix"),
        "$m = require(\"./lib1.mix\")\nprint($m.context())\n",
        &cfg,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_nested_require_resolves_next_to_its_module() {
    let dir = tree(
        "nested",
        &[
            (
                "a.mix",
                "$b = require(\"./sub/b.mix\")\nfn fa()\n  return 1\nend\n",
            ),
            ("sub/b.mix", "fn fb($x)\n  return $x\nend\n"),
        ],
    );
    let found = codes_at(
        &dir,
        "$a = require(\"./a.mix\")\nprint($a.b.fb())\nprint($a.b.nope(1))\nprint($a.b.fb(1))\nprint($a.fa())\n",
    );
    assert_eq!(
        found,
        vec![
            ("MIX-W2314".to_string(), Some(2)),
            ("MIX-W2315".to_string(), Some(3)),
        ]
    );
}

#[test]
fn a_require_cycle_terminates_and_still_checks() {
    let dir = tree(
        "cycle",
        &[
            (
                "a.mix",
                "$b = require(\"./b.mix\")\nfn fa($x)\n  return $x\nend\n",
            ),
            (
                "b.mix",
                "$a = require(\"./a.mix\")\nfn fb()\n  return 1\nend\n",
            ),
        ],
    );
    let found = codes_at(
        &dir,
        "$a = require(\"./a.mix\")\nprint($a.b.a.b.fb(1))\nprint($a.b.a.fa(1))\n",
    );
    assert_eq!(found, vec![("MIX-W2314".to_string(), Some(2))]);
}

#[test]
fn exports_follow_the_runtime_rules() {
    let dir = tree(
        "rules",
        &[(
            "m.mix",
            // A `$var` of a function's name wins the export; a function named
            // like a frame variable is still exported; a definition inside
            // another function exists only once that function runs.
            "fn helper($a, $b)\n  return $a\nend\n$helper = fn($x) = $x\nfn result($x)\n  return $x\nend\nfn setup()\n  fn later($a, $b)\n    return $a\n  end\nend\n",
        )],
    );
    let src =
        "$m = require(\"./m.mix\")\nprint($m.helper(7))\nprint($m.result(7))\nprint($m.later())\n";
    assert_eq!(codes_at(&dir, src), vec![]);
    let found = codes_at(&dir, "$m = require(\"./m.mix\")\nprint($m.result())\n");
    assert_eq!(found, vec![("MIX-W2314".to_string(), Some(2))]);
}

#[test]
fn calls_in_expression_bodies_and_defaults_are_checked() {
    let dir = tree("exprbody", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nfn f() = $m.nosuch()\nfn g($x = $m.helper(1)) = $x\n$h = fn() = $m.helper()\nprint(f() .. g() .. $h())\n";
    let found = codes_at(&dir, src);
    assert_eq!(
        found,
        vec![
            ("MIX-W2315".to_string(), Some(2)),
            ("MIX-W2314".to_string(), Some(3)),
            ("MIX-W2314".to_string(), Some(4)),
        ],
        "{found:?}"
    );
}

#[test]
fn a_call_in_an_if_expression_of_an_expression_body_is_checked() {
    let dir = tree("ifbody", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nfn f() = if true then $m.nosuch() end\nprint(f())\n";
    assert_eq!(
        codes_at(&dir, src),
        vec![("MIX-W2315".to_string(), Some(2))]
    );
}

#[test]
fn a_statement_member_call_has_no_ufcs_fallback() {
    // `$m.extra()` as a statement calls the field value: a caller `fn extra`
    // does not answer it, so the missing export still fails at run time.
    let dir = tree("stmtcall", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nfn extra($o)\n  return 1\nend\n$m.extra()\n$m.helper(1)\n$m.helper(1, 2)\n";
    assert_eq!(
        codes_at(&dir, src),
        vec![
            ("MIX-W2315".to_string(), Some(5)),
            ("MIX-W2314".to_string(), Some(6)),
        ]
    );
}

#[test]
fn a_rebinding_inside_an_expression_body_stands_down() {
    let dir = tree("exprrebind", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nfn f() = if true then $m = {helper: fn() = 7}; $m.helper() end\nprint(f())\n";
    assert_eq!(codes_at(&dir, src), vec![]);
}

#[test]
fn an_include_in_an_expression_body_stands_down() {
    let dir = tree(
        "exprinclude",
        &[
            ("lib1.mix", LIB1),
            ("more.mix", "fn greet()\n  return 1\nend\n"),
        ],
    );
    let src = "$m = require(\"./lib1.mix\")\nfn f() = if true then\n  include \"./more.mix\"\n  print($m.greet())\nend\nf()\n";
    let found = codes_at(&dir, src);
    assert!(
        !found
            .iter()
            .any(|(c, _)| c == "MIX-W2315" || c == "MIX-W2314"),
        "{found:?}"
    );
}

#[test]
fn a_fallback_function_inside_an_expression_body_answers_the_call() {
    // sol round 6: the program prints 7 — UFCS reaches the caller's `extra`.
    let dir = tree("exprfallback", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nfn f() = if true then\n  fn extra($obj) = 7\n  print($m.extra())\nend\nf()\n";
    let found = codes_at(&dir, src);
    assert!(!found.iter().any(|(c, _)| c == "MIX-W2315"), "{found:?}");
}

#[test]
fn a_piped_assignment_is_an_export_too() {
    // sol round 7: `$helper = … | cat` exports the lambda, which overrides
    // the two-parameter `fn helper`; 0.116 prints 7.
    let dir = tree(
        "piped",
        &[(
            "m.mix",
            "fn helper($a, $b) = $a\n$helper = fn($x) = $x | cat\n",
        )],
    );
    let src = "$m = require(\"./m.mix\")\nprint($m.helper(7))\n";
    assert_eq!(codes_at(&dir, src), vec![]);
    // The same unwrap keeps the undefined-variable check honest.
    let found = codes_at(&dir, "$v = 1 | cat\nprint($v)\n");
    assert!(!found.iter().any(|(c, _)| c == "MIX-E1101"), "{found:?}");
}

#[test]
fn a_call_inside_a_chain_or_pipe_is_reported_once_on_its_own_line() {
    // sol round 8: the wrapper and its operand were both checked.
    let dir = tree("wrappers", &[("lib1.mix", LIB1)]);
    let src = "$m = require(\"./lib1.mix\")\nprint($m.helper(1)) | cat\nprint(1) &&\n  print($m.nosuch())\n";
    assert_eq!(
        codes_at(&dir, src),
        vec![
            ("MIX-W2314".to_string(), Some(2)),
            ("MIX-W2315".to_string(), Some(4)),
        ]
    );
}

#[test]
fn the_require_checks_are_warnings_this_release() {
    let dir = tree("severity", &[("lib1.mix", LIB1)]);
    let found = lint_at(
        &dir.join("main.mix"),
        "$m = require(\"./lib1.mix\")\nprint($m.helper(1))\nprint($m.nosuch())\n",
        &AnalyzerConfig::default(),
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .all(|d| d.severity == mix::analyzer::Severity::Warning),
        "{found:?}"
    );
}

#[test]
fn a_module_in_a_status_variable_is_not_proven() {
    // `send` and every command rewrite `$result`, so a call through it may
    // reach a caller function by UFCS instead of the module.
    let dir = tree("status", &[("lib1.mix", LIB1)]);
    let src = "$result = require(\"./lib1.mix\")\nsend svc ping\nfn helper($o)\n  return 7\nend\nprint($result.helper())\n";
    let found = codes_at(&dir, src);
    assert!(
        !found
            .iter()
            .any(|(c, _)| c == "MIX-W2314" || c == "MIX-W2315"),
        "{found:?}"
    );
}

#[test]
fn a_nested_require_resolves_next_to_the_canonical_module() {
    // `entry/a.mix` is a symlink to `real/a.mix`; the runtime canonicalises
    // it, so `./b.mix` is `real/b.mix`, not `entry/b.mix`.
    let dir = tree(
        "symlink",
        &[
            ("real/a.mix", "$b = require(\"./b.mix\")\n"),
            ("real/b.mix", "fn fb($x)\n  return $x\nend\n"),
            ("entry/b.mix", "fn other()\n  return 1\nend\n"),
        ],
    );
    std::os::unix::fs::symlink(dir.join("real/a.mix"), dir.join("entry/a.mix")).unwrap();
    let found = codes_at(
        &dir,
        "$a = require(\"./entry/a.mix\")\nprint($a.b.fb(1))\nprint($a.b.fb())\n",
    );
    assert_eq!(found, vec![("MIX-W2314".to_string(), Some(3))]);
}

#[test]
fn a_function_named_after_a_shell_extension_warns() {
    let dir = tree("ext", &[]);
    let cfg = AnalyzerConfig {
        extensions: MIX_SHELL_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        ..AnalyzerConfig::default()
    };
    let src = "fn context($over)\n  return {state_dir: \"/srv\"}\nend\nprint(context(1))\n";
    let found = lint_at(&dir.join("ctxmod.mix"), src, &cfg);
    let codes: Vec<&str> = found.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec!["MIX-W2313"], "{found:?}");
    assert_eq!(found[0].line, Some(1));
    // A call to an extension is defined under the binary's lint...
    let found = lint_at(
        &dir.join("x.mix"),
        "print(context())\nprint(ai(\"q\"))\n",
        &cfg,
    );
    assert!(found.is_empty(), "{found:?}");
    // ...and an embedder that registers none keeps the old answer.
    let found = lint_at(
        &dir.join("x.mix"),
        "print(context())\n",
        &AnalyzerConfig::default(),
    );
    assert!(found.iter().any(|d| d.code == "MIX-E1102"), "{found:?}");
}

#[test]
fn a_module_exporting_context_is_called_through_the_member() {
    // The serve-citizen case: the export wins the member call, so the
    // caller is clean (the definition warns in the module's own lint).
    let dir = tree(
        "ctx",
        &[(
            "ctxmod.mix",
            "fn context($over)\n  return {state_dir: \"/srv\"}\nend\n",
        )],
    );
    let cfg = AnalyzerConfig {
        extensions: MIX_SHELL_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        ..AnalyzerConfig::default()
    };
    let found = lint_at(
        &dir.join("main.mix"),
        "$r = require(\"./ctxmod.mix\")\nprint($r.context({}).state_dir)\nprint($r.context())\n",
        &cfg,
    );
    let codes: Vec<&str> = found.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec!["MIX-W2314"], "{found:?}");
}
