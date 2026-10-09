// SPDX-License-Identifier: MIT OR Apache-2.0
//! Semantic analyzer (0.29.0, decision D3) — rule coverage AND, just as
//! load-bearing, the false-positive guards for Mix's dynamic seams.

use mix::analyzer::{AnalyzerConfig, Severity, analyze};
use mix::ast::{ChainOp, Expr, FunctionBody, PathSeg, Stmt, StmtKind};
use mix::lexer::Lexer;
use mix::parser::Parser;

fn lint(src: &str) -> Vec<(String, Option<usize>)> {
    lint_cfg(src, &AnalyzerConfig::default())
}

fn lint_cfg(src: &str, cfg: &AnalyzerConfig) -> Vec<(String, Option<usize>)> {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, src).parse_program().expect("parses");
    analyze(&stmts, Some("test.mix"), cfg)
        .diagnostics
        .into_iter()
        .map(|d| (d.code.to_string(), d.line))
        .collect()
}

fn codes(src: &str) -> Vec<String> {
    lint(src).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn streaming_http_contracts_include_filesystem_capabilities() {
    for (name,cap) in [("http_put_file","fs-read"),("http_get_file","fs-write")] {
        let src = format!("$r = {name}(\"https://example.invalid/file\", \"file\")\nprint($r.status)\n");
        let tokens = Lexer::new(&src).tokenize().unwrap();
        let stmts = Parser::new(tokens,&src).parse_program().unwrap();
        let a = analyze(&stmts,Some("test.mix"),&AnalyzerConfig::default());
        assert!(a.capabilities.contains(&"network"));
        assert!(a.capabilities.contains(&cap));
        let info = mix::builtins::builtin_info_of(name).unwrap();
        assert_eq!(info.signature(),format!("{name}(url, path[, opts]) -> map<http_file_response>"));
    }
}

/// Like `lint`, but keeps the code + hint so a test can pin the FIX a
/// diagnostic names, not just that it fired.
fn lint_full(src: &str) -> Vec<(String, Option<String>)> {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, src).parse_program().expect("parses");
    analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default())
        .diagnostics
        .into_iter()
        .map(|d| (d.code.to_string(), d.hint))
        .collect()
}

// ── detections ──────────────────────────────────────────────────────

#[test]
fn undefined_variable_including_nested_expressions() {
    // THE provisioning failure: a typo'd global inside a concatenation.
    let diags = lint(
        "function build_target($node)\n  return \"ct-\" .. $node .. \".\" .. $DOMAIN\nend\nbuild_target(\"x\")\n",
    );
    assert_eq!(diags, vec![("MIX-E1101".to_string(), Some(2))]);
}

#[test]
fn undefined_function_and_arities() {
    let out =
        codes("function f($a)\n  return $a\nend\nf(1, 2)\nsubstr(\"abc\")\nnope()\nrandom(1)\n");
    // W2201: `substr("abc")` alone is a discarded pure transform — it
    // returns a value and mutates nothing, so the statement is a no-op
    // (D3's must_use half, 2026-09-29).
    assert_eq!(
        out,
        vec!["MIX-E1202", "MIX-W2201", "MIX-E1201", "MIX-E1102", "MIX-E1201"]
    );
}

#[test]
fn duplicate_params_and_defs() {
    let out = codes(
        "function f($a, $a)\n  return 1\nend\nfunction g()\n  return 1\nend\nfunction g()\n  return 2\nend\n",
    );
    assert!(out.contains(&"MIX-E1301".to_string()));
    assert!(out.contains(&"MIX-E1302".to_string()));
}

#[test]
fn unreachable_and_must_use() {
    let out = lint("$r = run_rc(\"true\")\nrun_rc(\"true\")\nexit(0)\nprint(\"dead\")\n");
    let codes: Vec<&str> = out.iter().map(|(c, _)| c.as_str()).collect();
    assert!(codes.contains(&"MIX-W2201"));
    assert!(codes.contains(&"MIX-W2101"));
}

#[test]
fn require_missing_flags_e1401() {
    let out = codes("$m = require(\"/no/such/module-xyz.mix\")\nprint($m)\n");
    assert_eq!(out, vec!["MIX-E1401"]);
}

#[test]
fn list_addition_warns_for_literal_and_proven_variable() {
    assert_eq!(codes("$joined = [\"x\"] + [\"y\"]\n"), vec!["MIX-W2301"]);
    assert_eq!(
        codes("$items = [\"x\"]\n$joined = $items + [\"y\"]\n"),
        vec!["MIX-W2301"]
    );
}

/// 0.90.0: the same rule now covers MAP operands, which it never did —
/// `$m + {b: 2}` passed lint silently while the runtime string-concatenated
/// it, and now raises.
#[test]
fn map_addition_warns_and_names_merge() {
    assert_eq!(codes("$joined = {a: 1} + {b: 2}\n"), vec!["MIX-W2301"]);
    assert_eq!(
        codes("$m = {a: 1}\n$joined = $m + {b: 2}\n"),
        vec!["MIX-W2301"]
    );
    let (_, hint) = lint_full("$joined = {a: 1} + {b: 2}\n")
        .into_iter()
        .next()
        .expect("one diagnostic");
    assert!(
        hint.as_deref().unwrap_or("").contains("merge(map_a, map_b)"),
        "map+map must point at merge: {hint:?}"
    );
    // A map on ONE side is not a merge — the hint must not say it is.
    let (_, hint) = lint_full("$m = {a: 1}\n$joined = $m + \"x\"\n")
        .into_iter()
        .next()
        .expect("one diagnostic");
    assert!(
        !hint.as_deref().unwrap_or("").contains("merge("),
        "map+string must not suggest merge: {hint:?}"
    );
}

#[test]
fn list_addition_near_misses_stay_quiet() {
    assert_eq!(codes("$joined = \"a\" + \"b\"\n"), Vec::<String>::new());
    assert_eq!(
        codes("$items = []\n$items = \"a\"\n$joined = $items + \"b\"\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(
            "$items = []\nif env(\"USE_STRING\") != \"\" then\n  $items = \"a\"\nend\n$joined = $items + \"b\"\n"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn used_call_to_implicit_nil_function_warns() {
    let src = "function double($n)\n  $n * 2\nend\n$result = double(4)\n";
    assert_eq!(codes(src), vec!["MIX-W2302"]);
}

#[test]
fn discarded_implicit_nil_function_result_stays_quiet() {
    let src = "function double($n)\n  $n * 2\nend\ndouble(4)\n";
    assert_eq!(codes(src), Vec::<String>::new());
    let terminating = "function stop()\n  exit(0)\nend\n$result = stop()\n";
    assert_eq!(codes(terminating), Vec::<String>::new());
    let mixed = "function maybe($ok)\n  if $ok then\n    return 1\n  end\n  2\nend\n$result = maybe(true)\n";
    assert_eq!(codes(mixed), Vec::<String>::new());
}

#[test]
fn hand_built_assignment_chain_ast_keeps_w2303_defence() {
    let assignments = [
        StmtKind::Assignment {
            name: "x".into(),
            value: Expr::BoolLiteral(true),
        },
        StmtKind::FieldAssignment {
            object: "m".into(),
            field: "ok".into(),
            value: Expr::BoolLiteral(true),
        },
        StmtKind::IndexAssignment {
            object: "m".into(),
            index: Expr::StringLiteral("ok".into()),
            value: Expr::BoolLiteral(true),
        },
        StmtKind::PathAssignment {
            root: "m".into(),
            path: vec![PathSeg::Field("inner".into()), PathSeg::Field("ok".into())],
            value: Expr::BoolLiteral(true),
        },
    ];

    for assignment in assignments {
        for op in [ChainOp::And, ChainOp::Or] {
            for assignment_on_left in [true, false] {
                let assignment = Stmt::new(assignment.clone(), 1);
                let expression = Stmt::new(StmtKind::Expression(Expr::BoolLiteral(true)), 1);
                let (left, right) = if assignment_on_left {
                    (assignment, expression)
                } else {
                    (expression, assignment)
                };
                let chain = Stmt::new(
                    StmtKind::Chain {
                        left: Box::new(left),
                        op: op.clone(),
                        right: Box::new(right),
                    },
                    1,
                );
                let analysis =
                    analyze(&[chain], Some("hand-built.mix"), &AnalyzerConfig::default());
                let found = analysis
                    .diagnostics
                    .iter()
                    .find(|d| d.code == "MIX-W2303")
                    .expect("hand-built chain must retain W2303");
                // The warning must name the operator it actually saw: an `&&`
                // message on an `||` chain sends the reader hunting for an
                // operator that is not there.
                let (present, absent) = match op {
                    ChainOp::And => ("`&&`", "`||`"),
                    ChainOp::Or => ("`||`", "`&&`"),
                };
                assert!(
                    found.message.contains(present) && !found.message.contains(absent),
                    "W2303 must name {present}: {}",
                    found.message
                );
                assert!(
                    found
                        .hint
                        .as_deref()
                        .is_some_and(|h| h.contains("`and`") && h.contains("`or`")),
                    "W2303 hint must point at `and`/`or`: {:?}",
                    found.hint
                );
            }
        }
    }
}

/// The direct-operand cases above leave the analyser's *recursion* untested:
/// reverting either the `PipeToExternal` unwrapping or the descent into nested
/// Chains and statement bodies leaves them green. Each shape here is one the
/// parser cannot emit (it rejects first), so W2303 is only reachable through
/// the public `analyze()` API — which is exactly the defence being pinned.
#[test]
fn hand_built_assignment_chain_survives_wrapping_and_nesting() {
    let assignment = || {
        Stmt::new(
            StmtKind::Assignment {
                name: "x".into(),
                value: Expr::BoolLiteral(true),
            },
            1,
        )
    };
    let expression = || Stmt::new(StmtKind::Expression(Expr::BoolLiteral(true)), 1);
    let chain = |left: Stmt, right: Stmt| {
        Stmt::new(
            StmtKind::Chain {
                left: Box::new(left),
                op: ChainOp::And,
                right: Box::new(right),
            },
            1,
        )
    };

    // Operand wrapped in a pipeline: `$x = true | cat && true`.
    let piped = Stmt::new(
        StmtKind::PipeToExternal {
            stmt: Box::new(assignment()),
            command: "cat".into(),
        },
        1,
    );
    // Assignment buried in the deep left spine of a nested chain.
    let nested = chain(
        chain(chain(assignment(), expression()), expression()),
        expression(),
    );
    // Chain inside an `if` body — the analyser must descend into statement bodies.
    let in_body = Stmt::new(
        StmtKind::If {
            condition: Expr::BoolLiteral(true),
            then_body: vec![chain(assignment(), expression())],
            else_ifs: vec![],
            else_body: None,
        },
        1,
    );

    for (label, stmt) in [
        ("pipeline-wrapped operand", chain(piped, expression())),
        ("nested chain spine", nested),
        ("chain inside an if body", in_body),
    ] {
        let analysis = analyze(&[stmt], Some("hand-built.mix"), &AnalyzerConfig::default());
        assert!(
            analysis.diagnostics.iter().any(|d| d.code == "MIX-W2303"),
            "{label} must retain W2303"
        );
    }
}

#[test]
fn separate_assignment_and_or_chain_stays_quiet() {
    let src = "$ok = run_argv([\"true\"])\nrun(\"false\") || print(\"hi\")\n";
    assert_eq!(codes(src), Vec::<String>::new());
}

#[test]
fn separate_assignment_and_chain_stays_quiet() {
    let src = "$ok = run_argv([\"true\"])\nrun(\"true\") && print(\"hi\")\n";
    assert_eq!(codes(src), Vec::<String>::new());
}

#[test]
fn unknown_builtin_result_key_warns_with_real_key() {
    let src = "$r = run_argv([\"true\"])\nif $r[\"code\"] != 0 then\n  print(\"bad\")\nend\n";
    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let analysis = analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default());
    assert_eq!(analysis.diagnostics.len(), 1);
    let diagnostic = &analysis.diagnostics[0];
    assert_eq!(diagnostic.code, "MIX-W2304");
    assert!(
        diagnostic
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("use 'exit_code'"))
    );
}

#[test]
fn valid_or_unproven_result_map_keys_stay_quiet() {
    assert_eq!(
        codes("$r = run_argv([\"true\"])\nprint($r[\"exit_code\"])\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        codes("$r = make_result()\nprint($r[\"code\"])\n"),
        vec!["MIX-E1102"]
    );
}

#[test]
fn escaped_quote_in_ssh_command_string_warns_narrowly() {
    assert_eq!(
        codes(
            r#"$r = ssh_run("alpha", "print(\"remote\")")
"#
        ),
        vec!["MIX-W2306"]
    );
    assert_eq!(
        codes(
            r#"$out = ssh_must("alpha", "print(\"remote\")")
"#
        ),
        vec!["MIX-W2306"]
    );
    assert_eq!(
        codes("$r = ssh_run(\"alpha\", \"hostname\")\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(
            r#"$r = ssh_run("alpha", 'print("remote")')
"#
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(
            r#"$r = ssh_exec("alpha", ["print", "\"remote\""])
$m = ssh_mix("alpha", "print(\"remote\")")
"#
        ),
        Vec::<String>::new()
    );
}

// ── false-positive guards ───────────────────────────────────────────

#[test]
fn clean_provisioning_shaped_script_is_clean() {
    let src = "\
$job = validate({node: \"n\", vmid: 120}, {node: {nonblank: true}, vmid: {type: \"integer\"}})
$target = \"ct-\" .. $job.node
$r = run_argv([\"echo\", $target])
if not $r.ok then
  eprint(\"failed: \" .. $r.stderr)
  exit(1)
end
print($r.stdout)
";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn dynamic_include_suppresses_name_checks() {
    let out = lint("source \"./helpers.mix\"\nprint($from_helpers)\nhelper_fn()\n");
    assert_eq!(out, vec![("MIX-W2401".to_string(), Some(1))]);
}

#[test]
fn function_valued_variable_bareword_call_ok() {
    let src = "$greet = function($n) = \"hi \" .. $n\nprint(greet(\"x\"))\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn address_block_sends_are_not_undefined_functions() {
    let src = "address \"noded.delta.bus\"\n  some_remote_verb(1)\nend\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn injected_and_positional_and_interp_names_ok() {
    let src = "run(\"true\")\nprint($rc)\nprint($1)\nprint(\"${HOME}\")\nprint(\"${undefined_env_thing}\")\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn heredoc_ast_keeps_its_provenance() {
    let src = "$s = <<EOF\nplain\nEOF\n";
    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let mix::ast::StmtKind::Assignment { value, .. } = &stmts[0].kind else {
        panic!("expected assignment");
    };
    assert!(matches!(value, mix::ast::Expr::Heredoc(_)));
}

#[test]
fn bare_bound_variable_in_heredoc_warns_w2402() {
    // No `${...}` part is present: a literal-only heredoc must retain
    // its Heredoc AST provenance and still trigger the warning.
    let src = "$IP = \"192.0.2.1\"\n$s = <<EOF\nBare=$IP\nEOF\nprint($s)\n";
    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let a = analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default());
    assert_eq!(a.diagnostics.len(), 1);
    let d = &a.diagnostics[0];
    assert_eq!(d.code, "MIX-W2402");
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(
        d.message,
        "`$IP` in heredoc is not interpolated — did you mean `${IP}`?"
    );
    assert_eq!(
        d.hint.as_deref(),
        Some("literal `$IP` output requires no change")
    );
}

#[test]
fn heredoc_bare_variable_warning_false_positive_guards() {
    assert_eq!(
        lint("$IP = \"192.0.2.1\"\n$s = <<EOF\nAddr=${IP}/24\nEOF\nprint($s)\n"),
        vec![]
    );
    assert_eq!(
        lint("$IP = \"192.0.2.1\"\n$s = <<EOF\nBare=$notavar\nEOF\nprint($s)\n"),
        vec![]
    );
    assert_eq!(
        lint("$IP = \"192.0.2.1\"\n$s = <<EOF\nAwk=$1\nEOF\nprint($s)\n"),
        vec![]
    );
    assert_eq!(
        lint("$IP = \"192.0.2.1\"\n$s = \"Bare=$IP\"\nprint($s)\n"),
        vec![]
    );
    assert_eq!(
        lint("$IP = \"192.0.2.1\"\n$s = <<EOF\nEscaped=\\$IP\nEOF\nprint($s)\n"),
        vec![]
    );
}

#[test]
fn prelude_functions_recognized() {
    // Prelude defines helper fns; calling one must not be E1102 (pick
    // one from the embedded prelude if present — resilient check: the
    // prelude set is non-empty and each member lints clean).
    let names = mix::analyzer::prelude_function_names();
    if let Some(name) = names.iter().next() {
        let src = format!("{name}()\n");
        let out = lint(&src);
        assert!(
            out.iter().all(|(c, _)| c != "MIX-E1102"),
            "prelude fn {name} flagged: {out:?}"
        );
    }
}

#[test]
fn no_lexical_order_no_block_scoping() {
    let src = "\
function uses_late_global()
  return $config
end
if true then
  $inside_if = 1
end
print($inside_if)
$config = {a: 1}
print(uses_late_global())
for each $item in [1, 2]
  $last = $item
end
print($last)
try
  die(\"x\")
catch $msg, $err
  print($msg .. $err.code)
end
";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn allow_flags_respected() {
    let cfg = AnalyzerConfig {
        allow_globals: vec!["EXTERNAL".to_string()],
        allow_functions: vec!["embedder_fn".to_string()],
        // Only the nested ssh_mix-body analysis sets this (v0.69.0).
        ..AnalyzerConfig::default()
    };
    assert_eq!(lint_cfg("print($EXTERNAL)\nembedder_fn(1)\n", &cfg), vec![]);
}

#[test]
fn lambda_params_and_captures_ok() {
    let src = "$base = 10\n$xs = map([1, 2], function($x) = $x + $base)\nprint($xs)\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn must_use_as_last_statement_of_function_is_not_w2201() {
    let src = "function probe($h)\n  ssh_run($h, \"true\")\nend\n$r = probe(\"h\")\nprint($r)\n";
    assert_eq!(lint(src), vec![("MIX-W2302".to_string(), Some(4))]);
}

#[test]
fn variadic_and_optional_builtin_arities_ok() {
    let src = "print(fmt(\"%s %s\", 1, 2))\nprint(min(1, 2, 3, 4))\nprint(substr(\"abc\", 1))\nprint(random())\nprint(random(1, 2))\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn capabilities_inventory() {
    let tokens = Lexer::new(
        "$r = run_rc(\"true\")\n$h = http_get(\"https://192.0.2.1/\")\nprint($r.rc .. $h.status)\n",
    )
    .tokenize()
    .unwrap();
    let stmts = Parser::new(tokens, "x").parse_program().unwrap();
    let a = analyze(&stmts, None, &AnalyzerConfig::default());
    assert!(a.capabilities.contains(&"process"), "{:?}", a.capabilities);
    assert!(a.capabilities.contains(&"network"), "{:?}", a.capabilities);
}

// ── codex release-review MAJOR: embedded statement bodies ───────────

#[test]
fn if_expression_branches_are_scope_checked() {
    // false negative fix: undefined names inside an if-EXPRESSION branch.
    let out = codes("$x = if true then $undefined_in_branch else 0 end\nprint($x)\n");
    assert_eq!(out, vec!["MIX-E1101"]);
}

#[test]
fn var_assigned_in_if_expression_then_read_is_ok() {
    // false positive fix: a top-level var bound inside an if-expr branch
    // is in the file universe (no lexical order, no block scoping).
    let src = "$y = if true then\n  $inner = 5\n  $inner\nelse\n  0\nend\nprint($inner + $y)\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn function_defined_and_called_inside_block_lambda_is_ok() {
    // false positive fix: a def inside a block lambda is a real callable.
    let src = "$run = function()\n  function helper()\n    return 7\n  end\n  return helper()\nend\nprint($run())\n";
    assert_eq!(lint(src), vec![]);
}

#[test]
fn source_inside_block_lambda_degrades_to_w2401() {
    let out =
        codes("$f = function()\n  source \"./x.mix\"\n  return $from_source\nend\nprint($f())\n");
    assert_eq!(out, vec!["MIX-W2401"]);
}

#[test]
fn param_default_expression_is_scope_checked() {
    // false negative fix: undefined name in a parameter default.
    let out = codes("function f($a = $undefined_default)\n  return $a\nend\nf()\n");
    assert!(out.contains(&"MIX-E1101".to_string()), "{out:?}");
}

#[test]
fn non_trailing_default_arity_in_lint() {
    // matches the evaluator: min = past last required param.
    let out = codes("function f($a = 1, $b)\n  return $b\nend\nf(9)\n");
    assert_eq!(out, vec!["MIX-E1202"]);
}

// ── codex convergence review: analyzer-fix regressions ──────────────

#[test]
fn lambda_local_binding_does_not_leak_to_file_universe() {
    // A name bound only inside a lambda body is NOT visible at top level
    // (isolated frame) — reading it there is E1101.
    let out = codes("$f = function()\n  $lambda_local = 1\nend\nprint($lambda_local)\n");
    assert_eq!(out, vec!["MIX-E1101"]);
}

#[test]
fn function_local_binding_does_not_leak_to_file_universe() {
    let out = codes("function f()\n  $fn_local = 1\nend\nf()\nprint($fn_local)\n");
    assert_eq!(out, vec!["MIX-E1101"]);
}

#[test]
fn duplicate_defs_and_dead_code_inside_lambda_are_flagged() {
    let out = codes(
        "$f = function()\n  function helper()\n    return 1\n  end\n  function helper()\n    return 2\n  end\n  return helper()\nend\nprint($f())\n",
    );
    assert!(out.contains(&"MIX-E1302".to_string()), "{out:?}");
    let out = codes("$g = function()\n  return 1\n  print(\"dead\")\nend\nprint($g())\n");
    assert!(out.contains(&"MIX-W2101".to_string()), "{out:?}");
}

#[test]
fn deeply_nested_lambdas_do_not_blow_up() {
    // codex convergence review MAJOR: nested block lambdas were
    // O(2^depth). Depth 24 must analyze in well under a second.
    let mut src = String::new();
    for i in 0..24 {
        src.push_str(&format!("$f{i} = function()\n"));
    }
    src.push_str("  $x = 1\n");
    for _ in 0..24 {
        src.push_str("end\n");
    }
    let start = std::time::Instant::now();
    let _ = lint(&src);
    assert!(
        start.elapsed().as_millis() < 500,
        "nested-lambda analysis took {}ms",
        start.elapsed().as_millis()
    );
}

#[test]
fn severity_partition_is_stable() {
    let tokens = Lexer::new("nope()\n").tokenize().unwrap();
    let stmts = Parser::new(tokens, "x").parse_program().unwrap();
    let a = analyze(&stmts, None, &AnalyzerConfig::default());
    assert!(a.diagnostics.iter().all(|d| match d.code {
        c if c.starts_with("MIX-E") => d.severity == Severity::Error,
        c if c.starts_with("MIX-W") => d.severity == Severity::Warning,
        _ => false,
    }));
}

// ── E1501 / E1502: statements whose entire effect is provably lost ────
//
// Both are ERRORS, not warnings: the statement does nothing at all while
// reading as though it did. `push($m[$k], $v)` in particular is the
// spelling every newcomer reaches for, and before 0.33.0 it lint-passed
// clean while silently dropping the write.

#[test]
fn e1501_dead_push_into_a_container_element() {
    assert!(
        codes("$m = { a: [1] }\npush($m[\"a\"], 2)\nprint($m)\n")
            .contains(&"MIX-E1501".to_string())
    );
    assert!(
        codes("$m = { a: [1] }\npush($m.a, 2)\nprint($m)\n").contains(&"MIX-E1501".to_string())
    );
    // UFCS spelling desugars to the same call.
    assert!(
        codes("$m = { a: [1] }\n$m[\"a\"].push(2)\nprint($m)\n").contains(&"MIX-E1501".to_string())
    );
    assert!(
        codes("$m = { a: [1] }\npop($m[\"a\"])\nprint($m)\n").contains(&"MIX-E1501".to_string())
    );
}

#[test]
fn e1501_silent_on_the_forms_that_actually_work() {
    // A bare variable IS the mutable slot — this is the whole contract.
    assert!(!codes("$l = [1]\npush($l, 2)\nprint($l)\n").contains(&"MIX-E1501".to_string()));
    // The approved idiom: assign the returned list back through the path.
    assert!(
        !codes("$m = { a: [1] }\n$m[\"a\"] = push($m[\"a\"], 2)\nprint($m)\n")
            .contains(&"MIX-E1501".to_string())
    );
    // Result used → pop/shift on an expression are legitimate.
    assert!(
        !codes("$m = { a: [1] }\n$x = pop($m[\"a\"])\nprint($x)\n")
            .contains(&"MIX-E1501".to_string())
    );
    // A by-value PARAMETER is a bare variable: that dead-push case has
    // its own 0.21.9 diagnostic and must not be double-reported here.
    assert!(
        !codes("function f($p)\n  push($p, 1)\n  print($p)\nend\n")
            .contains(&"MIX-E1501".to_string())
    );
}

#[test]
fn e1502_discarded_pure_transform() {
    assert!(
        codes("$m = { a: 1 }\ndelete($m, \"a\")\nprint($m)\n").contains(&"MIX-E1502".to_string())
    );
    assert!(codes("$m = {}\nmerge($m, { b: 2 })\nprint($m)\n").contains(&"MIX-E1502".to_string()));
    // Assigned back → correct, and silent.
    assert!(
        !codes("$m = { a: 1 }\n$m = delete($m, \"a\")\nprint($m)\n")
            .contains(&"MIX-E1502".to_string())
    );
}

/// The analyser's operand predicate is documented as being kept in lockstep
/// with the parser's. The parser's copy is pinned by
/// `assignment_chain_parse.rs`; without this the analyser's copy could quietly
/// drop a form and the W2303 embedder defence would go silent while E1002
/// still fired — a divergence no source-text test can catch, because source
/// text never reaches the analyser once the parser rejects it.
#[test]
fn hand_built_value_binding_keyword_operands_keep_w2303() {
    let expression = || Stmt::new(StmtKind::Expression(Expr::BoolLiteral(true)), 1);
    let chain = |left: Stmt| {
        Stmt::new(
            StmtKind::Chain {
                left: Box::new(left),
                op: ChainOp::Or,
                right: Box::new(expression()),
            },
            1,
        )
    };

    let binders = [
        (
            "export",
            StmtKind::Export {
                name: "x".into(),
                value: Expr::BoolLiteral(false),
            },
        ),
        (
            "alias define",
            StmtKind::Alias {
                name: Some(Expr::StringLiteral("xx".into())),
                command: Some(Expr::StringLiteral("false".into())),
            },
        ),
        (
            "terse function def",
            StmtKind::FunctionDef {
                name: "f".into(),
                params: vec![],
                body: FunctionBody::Expression(Expr::BoolLiteral(false)),
            },
        ),
    ];

    for (label, kind) in binders {
        let analysis = analyze(
            &[chain(Stmt::new(kind, 1))],
            Some("hand-built.mix"),
            &AnalyzerConfig::default(),
        );
        assert!(
            analysis.diagnostics.iter().any(|d| d.code == "MIX-W2303"),
            "{label} operand must retain W2303"
        );
    }

    // The block-bodied form binds no `=` expression and must stay quiet.
    let block = StmtKind::FunctionDef {
        name: "f".into(),
        params: vec![],
        body: FunctionBody::Block(vec![expression()]),
    };
    let analysis = analyze(
        &[chain(Stmt::new(block, 1))],
        Some("hand-built.mix"),
        &AnalyzerConfig::default(),
    );
    assert!(
        !analysis.diagnostics.iter().any(|d| d.code == "MIX-W2303"),
        "block-bodied function def must stay a legal operand"
    );
}

// ── MIX-D3014: write_file of an unchecked replace() result (0.90.0) ──

#[test]
fn unguarded_edit_chain_notes_both_the_nested_and_the_stepwise_shape() {
    // The nested one-liner — the `mix -c` edit shape that shipped a commit
    // which did not compile on 2026-09-18.
    assert!(
        codes("write_file(\"/tmp/f\", replace(read_file(\"/tmp/f\"), \"a\", \"b\"))\n")
            .contains(&"MIX-D3014".to_string())
    );
    // The same chain spread over statements, which is how scripts write it.
    assert!(
        codes("$s = read_file(\"/tmp/f\")\n$s = replace($s, \"a\", \"b\")\nwrite_file(\"/tmp/f\", $s)\n")
            .contains(&"MIX-D3014".to_string())
    );
    // Regex twin.
    assert!(
        codes("$s = read_file(\"/tmp/f\")\n$s = re_replace($s, \"a+\", \"b\")\nwrite_file(\"/tmp/f\", $s)\n")
            .contains(&"MIX-D3014".to_string())
    );
}

#[test]
fn a_guarded_or_must_edit_chain_stays_quiet() {
    // A `contains` test anywhere means the author has the habit.
    assert!(
        !codes("$s = read_file(\"/tmp/f\")\nif contains($s, \"a\") then\n  $s = replace($s, \"a\", \"b\")\nend\nwrite_file(\"/tmp/f\", $s)\n")
            .contains(&"MIX-D3014".to_string())
    );
    // The `_must` twin IS the fix — it must not be the thing that is flagged.
    assert!(
        !codes("write_file(\"/tmp/f\", replace_must(read_file(\"/tmp/f\"), \"a\", \"b\"))\n")
            .contains(&"MIX-D3014".to_string())
    );
    // A write_file of something unrelated is not an edit chain.
    assert!(
        !codes("$s = \"hello\"\nwrite_file(\"/tmp/f\", $s)\n")
            .contains(&"MIX-D3014".to_string())
    );
    // Reassignment from a non-replace clears the fact.
    assert!(
        !codes("$s = replace(\"x\", \"a\", \"b\")\n$s = \"literal\"\nwrite_file(\"/tmp/f\", $s)\n")
            .contains(&"MIX-D3014".to_string())
    );
}

#[test]
fn d3014_is_a_note_so_it_never_gates_deny_warnings() {
    let out = lint("write_file(\"/tmp/f\", replace(read_file(\"/tmp/f\"), \"a\", \"b\"))\n");
    assert!(out.iter().any(|(c, _)| c == "MIX-D3014"));
    let tokens = Lexer::new("write_file(\"/tmp/f\", replace(read_file(\"/tmp/f\"), \"a\", \"b\"))\n")
        .tokenize()
        .unwrap();
    let stmts = Parser::new(
        tokens,
        "write_file(\"/tmp/f\", replace(read_file(\"/tmp/f\"), \"a\", \"b\"))\n",
    )
    .parse_program()
    .unwrap();
    let analysis = analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default());
    let d = analysis
        .diagnostics
        .iter()
        .find(|d| d.code == "MIX-D3014")
        .unwrap();
    assert_eq!(d.severity, Severity::Note);
}

// ── MIX-D3015 / MIX-W2405: how a double-quoted literal was SPELLED ──
//
// Both need the source text (`AnalyzerConfig::source`) because `'$sp/x'`
// and `"$sp/x"` lex to the same `Token::String` and the AST must not grow
// a variant to say which — `Token::InterpString` is a hard
// StrictDataViolation, so the shape change would refuse data files that
// work today.

fn codes_with_source(src: &str) -> Vec<String> {
    let cfg = AnalyzerConfig {
        source: Some(src.to_string()),
        ..AnalyzerConfig::default()
    };
    lint_cfg(src, &cfg).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn bare_dollar_in_a_double_quoted_string_notes_when_the_name_is_bound() {
    // The filing repro: four occurrences in one file passed lint and all
    // four failed at runtime.
    assert_eq!(
        codes_with_source("$sp = \"/tmp/x\"\nprint(read_file(\"$sp/prompt.md\"))\n"),
        vec!["MIX-D3015"]
    );
}

#[test]
fn bare_dollar_near_misses_stay_quiet() {
    for src in [
        // The three clean spellings the hint offers.
        "$sp = \"/tmp/x\"\nprint(\"${sp}/x\")\n",
        "$sp = \"/tmp/x\"\nprint('$sp/x')\n",
        "$sp = \"/tmp/x\"\nprint(\"\\$sp/x\")\n",
        // A name bound nowhere is prose, not a mistake.
        "print(\"Total: $USD\")\n",
        // `$` followed by a non-identifier, or by digits only, is not a
        // variable spelling.
        "$x = 1\nprint(\"cost $5.00 plus $ tax\")\n",
        // NESTED source: a multi-line string, or one whose spelling has
        // `\"`, carries an inner program whose `$rc` is correctly literal.
        "$rc = 1\nprint(\"print($rc)\\nprint(1)\")\n",
        "$CMCTL = \"/x\"\nprint(\"$CMCTL .. \\\"/etc\\\"\")\n",
    ] {
        assert!(
            !codes_with_source(src).contains(&"MIX-D3015".to_string()),
            "must stay quiet: {src}"
        );
    }
}

#[test]
fn unknown_escapes_warn_and_the_deliberate_literals_do_not() {
    assert_eq!(codes_with_source("print(\"bad \\q\")\n"), vec!["MIX-W2405"]);
    // `\x` with fewer than two hex digits is the whole point of keeping it
    // literal rather than a lex error.
    assert_eq!(codes_with_source("print(\"\\x4\")\n"), vec!["MIX-W2405"]);
    assert_eq!(codes_with_source("print(\"\\'\")\n"), vec!["MIX-W2405"]);
    for src in [
        // Added in 0.90.0 — these must NOT warn.
        "print(\"\\x27\")\n",
        "print(\"\\0\")\n",
        "print(\"\\a\\b\\f\\v\")\n",
        // Always recognised.
        "print(\"\\n\\t\\r\\e\\\"\\\\\\$\\~\")\n",
        "print(\"\\u{27}\")\n",
        // Unbraced `\u` is a DOCUMENTED literal (embedded JSON, C:\users).
        "print(\"json \\uABCD\")\n",
        // Single quotes keep their own rules and are not scanned.
        "print('raw \\x27')\n",
    ] {
        assert!(
            !codes_with_source(src).contains(&"MIX-W2405".to_string()),
            "must stay quiet: {src}"
        );
    }
}

#[test]
fn the_spelling_rules_are_silent_without_a_source() {
    // An embedder calling analyze() without AnalyzerConfig::source loses
    // these two rules and nothing else.
    assert!(codes("$sp = \"/tmp/x\"\nprint(\"$sp/x\")\nprint(\"bad \\q\")\n").is_empty());
}

// ── Two-arm cold review, round 1 (2026-09-21) ───────────────────────
//
// Five findings against the 0.90.0 lint additions, each pinned here so
// the fix cannot regress quietly.

#[test]
fn d3014_sees_inside_an_if_expression_both_ways() {
    // `walk_expr_children` skips Expr::If entirely, so the branch bodies
    // and the CONDITION were invisible to both halves of the rule: the
    // unchecked write went unreported, and a `contains()` guard written
    // in a condition did not silence anything.
    assert!(
        codes("$x = if true then\n  write_file(\"/dev/null\", replace(\"a\", \"a\", \"b\"))\nelse\n  nil\nend\n")
            .contains(&"MIX-D3014".to_string()),
        "an unchecked write inside an if-EXPRESSION branch must still be seen"
    );
    assert!(
        !codes("$g = if contains(\"a\", \"a\") then 1 else 2 end\nwrite_file(\"/dev/null\", replace(\"a\", \"a\", \"b\"))\n")
            .contains(&"MIX-D3014".to_string()),
        "a guard in an if-EXPRESSION condition must silence the file"
    );
}

#[test]
fn d3014_facts_do_not_cross_a_function_frame_or_survive_a_branch() {
    // A parameter SHADOWS the enclosing binding, so an outer
    // `$s = replace(...)` says nothing about the `$s` inside `save`.
    assert!(
        !codes("$s = replace(\"a\", \"a\", \"b\")\nfn save($s)\n  write_file(\"/dev/null\", $s)\nend\nsave(\"plain\")\n")
            .contains(&"MIX-D3014".to_string()),
        "a same-named parameter is a different variable"
    );
    // A conditional reassignment makes the fact UNKNOWN, not still-true.
    assert!(
        !codes("$s = replace(\"a\", \"a\", \"b\")\nif true then\n  $s = \"unrelated\"\nend\nwrite_file(\"/dev/null\", $s)\n")
            .contains(&"MIX-D3014".to_string()),
        "a branch that rebinds the name must invalidate the fact"
    );
}

#[test]
fn d3015_covers_function_parameters_not_only_top_level_names() {
    // `top_level_names` excludes parameters by design, and a helper's own
    // `print("$p/file")` is the commonest shape of this mistake.
    assert!(
        codes_with_source("fn sample($p)\n  print(\"$p/file\")\nend\nsample(\"x\")\n")
            .contains(&"MIX-D3015".to_string())
    );
    assert!(
        codes_with_source("$f = fn($q) print(\"$q/file\") end\n$f(\"x\")\n")
            .contains(&"MIX-D3015".to_string()),
        "a lambda parameter counts too"
    );
}

#[test]
fn an_escaped_physical_newline_is_located_and_named_safely() {
    // Three separate defects in one shape: the note was attributed to the
    // line AFTER the backslash (self.line had already advanced over the
    // newline), `multiline` was never set so the bare-$ batch survived in
    // a string that genuinely spans lines, and the message quoted the
    // escape verbatim — putting a raw newline inside a diagnostic and
    // splitting one finding across two output lines.
    let src = "$x = 1\nprint(\"x\\\n$x\")\n";
    let out = lint_cfg(
        src,
        &AnalyzerConfig {
            source: Some(src.to_string()),
            ..AnalyzerConfig::default()
        },
    );
    let codes: Vec<&str> = out.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(codes, vec!["MIX-W2405"], "no D3015: the string is multi-line");
    assert_eq!(out[0].1, Some(2), "the backslash is on line 2, not line 3");

    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let analysis = analyze(
        &stmts,
        Some("test.mix"),
        &AnalyzerConfig {
            source: Some(src.to_string()),
            ..AnalyzerConfig::default()
        },
    );
    for d in &analysis.diagnostics {
        let text = format!("{}{}", d.message, d.hint.clone().unwrap_or_default());
        assert!(
            !text.contains('\n') && !text.contains('\r'),
            "no diagnostic text may contain a raw line break: {text:?}"
        );
    }
}

#[test]
fn d3015_parameter_visibility_is_scoped_to_the_function() {
    // Round 2 caught the cost of the file-wide parameter union: an
    // unrelated helper's parameter made ordinary prose a finding.
    assert!(
        !codes_with_source(
            "fn unrelated($price)\n  return $price\nend\nprint(\"The price is $price per item\")\nunrelated(1)\n"
        )
        .contains(&"MIX-D3015".to_string()),
        "a parameter must not be visible outside its own function"
    );
    // ...while the in-scope case it was added for still fires, including
    // across a multi-line body and inside a lambda.
    assert!(
        codes_with_source("fn helper($dir)\n  $a = 1\n  print(\"$dir/sub\")\n  return $a\nend\nhelper(\"x\")\n")
            .contains(&"MIX-D3015".to_string())
    );
    assert!(
        codes_with_source("$f = fn($q) print(\"$q/file\") end\n$f(\"x\")\n")
            .contains(&"MIX-D3015".to_string())
    );
}

#[test]
fn shell_command_as_bare_string_is_an_error() {
    // B2: `hostname` on PATH as a bare string statement is the shell
    // reflex — nothing runs, exit 0, lint used to print 0/0/0.
    let out = codes("hostname\n");
    assert!(out.contains(&"MIX-E1507".to_string()), "got: {out:?}");
    // A bare string whose head is NOT on PATH stays silent (prose/value).
    let out = codes("not-a-command-anywhere-xyz\n");
    assert!(!out.contains(&"MIX-E1507".to_string()), "got: {out:?}");
}

#[test]
fn send_result_never_checked_warns() {
    let out = codes("send svc ping\nsend svc ping\n");
    assert!(out.contains(&"MIX-W2307".to_string()), "got: {out:?}");
    // Reading $rc after EACH send satisfies the check.
    let out = codes("send svc ping\nprint($rc)\nsend svc ping\nprint($rc)\n");
    assert!(!out.contains(&"MIX-W2307".to_string()), "got: {out:?}");
}

#[test]
fn send_status_read_in_condition_map_or_nested_arg_counts() {
    // Reproducer: `$rc` is read in an if-condition and inside a map value
    // nested in a call argument. Both used to be invisible to the scan,
    // so a correctly checked send warned.
    let reproducer = "$target=\"svc\"\n$verb=\"ping\"\n$payload=\"{}\"\n\
        send $target $verb body=$payload timeout=45\n\
        if $rc != 0 then exit(1) end\n\
        print(json_encode({rc:$rc,result:$result}))\n";
    let out = codes(reproducer);
    assert!(
        !out.contains(&"MIX-W2307".to_string()),
        "reproducer: got: {out:?}"
    );

    // Each read form on its own satisfies the check.
    for read in [
        "if $rc != 0 then print(1) end\n",
        "print(json_encode({rc:$rc}))\n",
        "print(json_encode({outer:{inner:[$result]}}))\n",
        "print(\"status=${rc}\")\n",
        "$ok = ($rc == 0)\n",
    ] {
        let src = format!("send svc ping\n{read}");
        let out = codes(&src);
        assert!(
            !out.contains(&"MIX-W2307".to_string()),
            "read `{read}` should count: got: {out:?}"
        );
    }
}

#[test]
fn send_status_check_still_warns_when_unread_and_stops_at_next_send() {
    // Truly unchecked: nothing reads the status at all.
    let out = codes("send svc ping\nprint(json_encode({ok:true}))\n");
    assert!(out.contains(&"MIX-W2307".to_string()), "got: {out:?}");

    // A read BEFORE the next send does not cover the first send's status
    // when the second send is the one that is never checked.
    let out = codes("send svc ping\nsend svc ping\nprint($rc)\n");
    let count = out.iter().filter(|c| *c == "MIX-W2307").count();
    assert_eq!(count, 1, "first send unchecked: got: {out:?}");

    // A read AFTER the next send does not count for the earlier send.
    let out = codes("send svc ping\nsend svc ping\nif $rc != 0 then exit(1) end\n");
    let count = out.iter().filter(|c| *c == "MIX-W2307").count();
    assert_eq!(count, 1, "read after the next send: got: {out:?}");
}

#[test]
fn send_status_overwritten_by_an_expression_send_still_warns_on_the_first() {
    // An expression-position send overwrites $rc too, so it ends the scan
    // for the first send. The read AFTER it checks the second send only:
    // the first send's failure is still never checked, and must warn.
    let out = codes("send svc first\n$x = send svc second\nif $rc != 0 then exit(1) end\n");
    let count = out.iter().filter(|c| *c == "MIX-W2307").count();
    assert_eq!(count, 1, "first send unchecked: got: {out:?}");

    // The statement's own reads run before its send, so they check the
    // first send's status.
    let out = codes("send svc first\n$x = send svc second k=$rc\n");
    assert!(!out.contains(&"MIX-W2307".to_string()), "got: {out:?}");
}

/// Lines of the MIX-W2307 diagnostics, in report order.
fn w2307_lines(src: &str) -> Vec<Option<usize>> {
    let tokens = Lexer::new(src).tokenize().expect("lexes");
    let stmts = Parser::new(tokens, src).parse_program().expect("parses");
    analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default())
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "MIX-W2307")
        .map(|d| d.line)
        .collect()
}

#[test]
fn send_status_send_inside_an_if_expression_overwrites_before_the_read() {
    // The second send runs inside the if-expression body and overwrites
    // $rc before the read after the if. The first send's status is never
    // checked, so it must warn (line 1). The second send reads $rc in its
    // own body, so only the first warns.
    let src = "send svc first\n$x = if true then\nsend svc second\n$rc\nelse\n0\nend\n\
               if $rc != 0 then exit(1) end\n";
    assert_eq!(w2307_lines(src), vec![Some(1)], "src: {src}");
}

#[test]
fn send_status_read_inside_an_if_expression_before_a_send_credits_the_first() {
    // The read runs in the body before the second send, so the first send's
    // status is checked. Only the second send, never read, warns (line 8).
    let src = "send svc first\n$x = if true then\nprint($rc)\n0\nelse\n0\nend\nsend svc second\n";
    assert_eq!(w2307_lines(src), vec![Some(8)], "src: {src}");
}

#[test]
fn send_status_nested_send_in_an_argument_is_not_credited_by_a_later_read() {
    // `b=$rc` runs after the nested send in `a=` has overwritten $rc, so it
    // reads the nested send's status, not the first send's. The first send
    // must warn (line 1).
    let src = "send svc first\nsend svc third a=if true then send svc second end b=$rc\n";
    assert!(
        w2307_lines(src).contains(&Some(1)),
        "got: {:?}",
        w2307_lines(src)
    );

    // Control: with no nested send, `b=$rc` checks the first send.
    let src = "send svc first\nsend svc third b=$rc\n";
    assert!(
        !w2307_lines(src).contains(&Some(1)),
        "got: {:?}",
        w2307_lines(src)
    );
}

#[test]
fn send_status_untaken_if_expression_branch_does_not_credit_the_first_send() {
    // The else arm is the only one taken (the condition is literal false), and
    // it sends again before any read, so the first send is never checked. The
    // read after the if must not credit it.
    let src = "send svc first\n$x = if false then $rc else send svc second end\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_read_in_only_one_if_branch_does_not_credit_the_first_send() {
    // Only the then arm reads. A path through the else arm reaches the end
    // unread, so the first send must warn.
    let src = "send svc first\nif $flag then\nprint($rc)\nelse\nprint(1)\nend\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_read_in_both_if_branches_credits_the_first_send() {
    // Control: every path reads the status before the code ends.
    let src = "send svc first\nif $flag then\nprint($rc)\nelse\nprint($rc)\nend\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_read_in_a_handler_body_does_not_credit_the_outer_send() {
    // The handler runs later, when a message arrives. Its read is not a read
    // of the status left by the first send, which is then overwritten.
    let src =
        "send svc first\non tick\nprint($rc)\nend\nsend svc second\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_send_inside_a_handler_body_is_checked_on_its_own() {
    // The send in the handler body has no read after it in that body.
    let src = "on tick\nsend svc first\nprint(2)\nend\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(2)), "got: {got:?}");
}

#[test]
fn send_status_read_inside_a_loop_body_does_not_credit_the_first_send() {
    // The loop may run zero times, so the read may never happen.
    let src = "send svc first\nfor $x in [1, 2, 3]\nprint($rc)\nend\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_exit_without_a_read_leaves_the_send_unchecked() {
    // The script ends with the status unread, so a failed send exits 0.
    let src = "send svc first\nexit(1)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_exit_after_a_read_is_checked() {
    // Control: the read comes before the exit on every path.
    let src = "send svc first\nprint($rc)\nexit(1)\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_nil_coalesce_right_side_does_not_credit_the_first_send() {
    // `??` skips its right side when the left is not nil, so the read of
    // $rc may never happen and the first send is unchecked.
    let src = "send svc first\n$x = 42 ?? $rc\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_nil_coalesce_left_side_read_credits_the_first_send() {
    // Control: the left side is always evaluated, so its read always happens.
    let src = "send svc first\n$x = $rc ?? 0\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_ternary_branch_does_not_credit_the_first_send() {
    // Only one branch of `c ? a : b` runs, so a read in one branch does not
    // decide every path.
    let src = "send svc first\n$x = $flag ? $rc : 0\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_publish_overwrites_before_the_read() {
    // publish sets $rc / $result / $reply like send, so the read after it
    // checks the publish, not the first send. The first send must warn.
    let src = "send svc first\n$published = publish(\"events\", \"payload\")\n$ok = ($rc == 0)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_publish_argument_read_credits_the_first_send() {
    // The arguments run before publish overwrites the status, so a read in
    // them checks the first send.
    let src = "send svc first\n$published = publish(\"events\", $rc)\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_shell_statement_overwrites_before_the_read() {
    // `sh "cmd"` sets $rc when the shell exits.
    let src = "send svc first\nsh \"true\"\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_shell_expression_overwrites_before_the_read() {
    // `$out = sh "cmd"` is the expression form, which also sets $rc.
    let src = "send svc first\n$out = sh \"true\"\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_command_substitution_overwrites_before_the_read() {
    // `$(cmd)` runs a shell and sets $rc.
    let src = "send svc first\n$out = $(true)\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_command_substitution_in_a_heredoc_overwrites_in_order() {
    // Heredoc parts run in order: a `$(...)` before a read of $rc overwrites
    // the status, so the read checks that command and not the first send.
    let src = "send svc first\n$s = <<EOF\n$(true)${rc}\nEOF\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
    // The other order: the read comes first and checks the first send.
    let src = "send svc first\n$s = <<EOF\n${rc}$(true)\nEOF\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_dollar_paren_in_a_double_quoted_string_is_literal() {
    // In a double-quoted string `$(` is literal text, not a command, so it
    // does not overwrite $rc: the read after it checks the first send.
    let src = "send svc first\n$s = \"$(true)${rc}\"\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_pipe_to_external_overwrites_before_the_read() {
    // The piped statement runs, then the external command sets $rc.
    let src = "send svc first\nprint(\"x\") | cat\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_assigning_a_status_name_overwrites_before_the_read() {
    // A user assignment to $rc replaces the status the send left behind.
    let src = "send svc first\n$rc = 0\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_for_each_binder_named_as_a_status_overwrites() {
    // A loop variable named $rc is written on each pass.
    let src = "send svc first\nfor $rc in [1, 2]\nprint(1)\nend\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_unknown_call_in_an_address_block_overwrites() {
    // Inside an address block an unknown call is sent to the address, which
    // sets $rc. The read after the block checks that call, not the first send.
    let src = "send svc first\naddress \"other\"\nping(1)\nend\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_read_inside_an_address_block_credits_the_first_send() {
    // An address body takes only calls. The read of $rc in the argument runs
    // before the call is sent, so it checks the first send.
    let src = "send svc first\naddress \"other\"\nping($rc)\nend\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_method_call_in_an_address_block_overwrites() {
    // A method call in an address block is an address send too.
    let src = "send svc first\naddress \"other\"\n$m.ping()\nend\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_call_to_a_function_that_sends_overwrites() {
    // A call does not save $rc, so the send inside f() overwrites the status
    // the first send left. The first send (line 4) must warn.
    let src =
        "function f()\n  send svc second\nend\nsend svc first\nf()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(4)), "got: {got:?}");
}

#[test]
fn send_status_call_to_a_function_that_only_sends_through_another_overwrites() {
    // outer() overwrites only through inner(), so the overwrite is transitive.
    let src = "function inner()\n  send svc second\n  if $rc != 0 then exit(1) end\nend\n\
               function outer()\n  inner()\nend\nsend svc first\nouter()\n\
               if $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(8)), "got: {got:?}");
}

#[test]
fn send_status_call_to_a_function_that_does_not_send_credits_the_first_send() {
    // g() never writes the status, so the read after the call checks the first send.
    let src = "function g()\n  print(1)\nend\nsend svc first\ng()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(4)), "got: {got:?}");
}

#[test]
fn send_status_emit_writes_no_status_and_credits_the_first_send() {
    // emit is fire-and-forget and documented to write no $rc / $result, so the
    // read after it still checks the first send.
    let src = "send svc first\nemit \"svc\" ping\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn push_assign_back_is_an_error() {
    let out = codes("$l = [1]\n$l = push($l, 2)\nprint($l)\n");
    assert!(out.contains(&"MIX-E1508".to_string()), "got: {out:?}");
    let out = codes("$l = [1]\npush($l, 2)\nprint($l)\n");
    assert!(!out.contains(&"MIX-E1508".to_string()), "got: {out:?}");
}

#[test]
fn reflex_surplus_arity_gets_the_mix_form_hint() {
    // A5: remove(map, key) — a surplus argument that is a Python hand —
    // carries the Mix-form hint on the E1201 diagnostic.
    let diags = lint_full("$m = {a: 1}\nremove($m, \"a\")\n");
    let e1201 = diags
        .iter()
        .find(|(c, _)| c == "MIX-E1201")
        .expect("E1201 fires");
    assert!(
        e1201.1.as_deref().is_some_and(|h| h.contains("delete(map, key)")),
        "reflex hint missing: {diags:?}"
    );
}

#[test]
fn literal_type_contradictions_are_errors() {
    // A2 lint half: a literal whose type cannot satisfy the contract.
    for src in ["mkdir({a: 1})\n", "exists([1, 2])\n", "len(3)\n", "write_file(99, \"x\")\n"] {
        let out = codes(src);
        assert!(out.contains(&"MIX-E1203".to_string()), "{src}: got {out:?}");
    }
    // Variables are not judged, and well-typed literals pass.
    let out = codes("exists(\".\")\nwrite_file($p, \"x\")\nmkdir($dir)\n");
    assert!(!out.contains(&"MIX-E1203".to_string()), "got: {out:?}");
}

#[test]
fn chain_operand_blocks_and_send_commands_are_walked() {
    // Lint-walker gaps (2026-09-24): a block statement used as a chain
    // operand, and a send command expression, are now body-walked —
    // `$nope` inside the if-block and the undefined fn in the command
    // are no longer invisible.
    let out = codes("if true then\n  print($nope)\nend && print(2)\n");
    assert!(out.contains(&"MIX-E1101".to_string()), "got: {out:?}");
    let out = codes("ssh_mix(\"h\", \"print(1)\")\nif true then\n  ssh_mix(\"h\", \"print(undefinedfn(1))\")\nend && print(2)\n");
    assert!(out.contains(&"MIX-E1102".to_string()), "got: {out:?}");
    // `$f(1)` in the command position: $f is an undefined VARIABLE, so
    // the walk produces E1101 — the point is that the command expr is
    // visited at all (it produced nothing before the fix).
    let out = codes("send \"svc\" $f(1)\n");
    assert!(out.contains(&"MIX-E1101".to_string()), "got: {out:?}");
}

// ── W2311: fmt/sprintf surplus operands (A5) ────────────────────────

#[test]
fn fmt_and_sprintf_surplus_operands_warn_with_statement_lines() {
    // Both variadic builtins silently ignore arguments past the template.
    let out = lint("print(fmt(\"%s\", 1, 2))\nprint(sprintf(\"%s\", 1, 2))\n");
    assert_eq!(
        out,
        vec![
            ("MIX-W2311".to_string(), Some(1)),
            ("MIX-W2311".to_string(), Some(2))
        ]
    );
}

#[test]
fn fmt_dynamic_width_consumes_an_operand_and_escaped_percent_consumes_none() {
    // `%*s` pulls TWO operands (width + value): exact is quiet, one extra
    // warns. `%%` is a literal percent — a placeholder-free template with
    // an argument is a surplus, without one it is exact.
    assert_eq!(lint("$x = fmt(\"%*s\", 5, \"hi\")\n"), vec![]);
    assert_eq!(
        lint("$x = fmt(\"%*s\", 5, \"hi\", \"extra\")\n"),
        vec![("MIX-W2311".to_string(), Some(1))]
    );
    assert_eq!(lint("$x = fmt(\"100%% done\")\n"), vec![]);
    assert_eq!(
        lint("$x = fmt(\"100%% done\", \"unused\")\n"),
        vec![("MIX-W2311".to_string(), Some(1))]
    );
}

#[test]
fn sprintf_dynamic_width_and_precision_consume_operands() {
    // A negative literal width operand still counts as ONE consumed
    // operand (it left-justifies); `%*.*f` pulls width + precision +
    // value = 3. Flags and length modifiers consume nothing extra.
    assert_eq!(lint("$x = sprintf(\"%*d\", -5, 42)\n"), vec![]);
    assert_eq!(
        lint("$x = sprintf(\"%*d\", -5, 42, \"extra\")\n"),
        vec![("MIX-W2311".to_string(), Some(1))]
    );
    assert_eq!(lint("$x = sprintf(\"%*.*f\", 8, 3, 1.5)\n"), vec![]);
    assert_eq!(
        lint("$x = sprintf(\"%*.*f\", 8, 3, 1.5, 0)\n"),
        vec![("MIX-W2311".to_string(), Some(1))]
    );
    assert_eq!(lint("$x = sprintf(\"%#+08.3llx\", 255)\n"), vec![]);
    assert_eq!(
        lint("$x = sprintf(\"%#+08.3llx\", 255, 2)\n"),
        vec![("MIX-W2311".to_string(), Some(1))]
    );
}

#[test]
fn fmt_and_sprintf_exact_and_deficits_stay_silent() {
    // Exact match and too FEW arguments (the runtime's own "not enough
    // arguments" error) produce no unused-operand warning.
    assert_eq!(lint("$x = fmt(\"%s %d\", \"a\", 1)\n"), vec![]);
    assert_eq!(lint("$x = fmt(\"%s %d\", \"a\")\n"), vec![]);
    assert_eq!(lint("$x = sprintf(\"%.2f|%s\", 3.14, \"ok\")\n"), vec![]);
    assert_eq!(lint("$x = sprintf(\"%s\")\n"), vec![]);
}

#[test]
fn invalid_or_unknown_templates_stay_silent() {
    // Each of these RAISES at runtime, so no definite surplus can be
    // claimed: %x is not a fmt conversion (it is for sprintf), %p is
    // unknown to both, a trailing % and fmt's unsupported `.*` and
    // digit-after-`*` are errors.
    assert_eq!(lint("$x = fmt(\"%s %x\", 1, 2, 3)\n"), vec![]);
    assert_eq!(lint("$x = sprintf(\"%s %p\", 1, 2)\n"), vec![]);
    assert_eq!(lint("$x = fmt(\"done %\", 1, 2)\n"), vec![]);
    assert_eq!(lint("$x = fmt(\"%*.*f\", 1, 2, 3, 4)\n"), vec![]);
    assert_eq!(lint("$x = fmt(\"%*5s\", 5, \"x\", \"y\")\n"), vec![]);
    // A dynamic (non-literal) template is never judged.
    let out = codes("$t = \"%s\"\n$x = fmt($t, 1, 2)\n");
    assert!(!out.contains(&"MIX-W2311".to_string()), "got: {out:?}");
}

#[test]
fn shadowed_fmt_and_sprintf_calls_stay_silent() {
    // A user function of the same name is its own MIX-E1303 error; the
    // surplus check stands down rather than double-diagnosing. A
    // variable of the same name (bareword dispatch ambiguity) silences
    // it outright.
    let out = lint("fn fmt($a, $b)\n  return \"$a $b\"\nend\n$r = fmt(\"%s\", 1, 2)\n");
    assert!(
        out.contains(&("MIX-E1303".to_string(), Some(1))),
        "the shadowing definition still errors: {out:?}"
    );
    assert!(
        !out.iter().any(|(c, _)| c == "MIX-W2311"),
        "a user-defined fmt silences the surplus warning: {out:?}"
    );
    let out = codes("$sprintf = \"custom\"\n$r = sprintf(\"%s\", 1, 2)\n");
    assert!(
        !out.contains(&"MIX-W2311".to_string()),
        "a $sprintf variable silences the surplus warning: {out:?}"
    );
}

#[test]
fn fmt_surplus_found_in_nested_expressions() {
    // Every call site is visited — list literals, concatenations,
    // if-expression branches — each reported on its STATEMENT's line.
    let src = "$a = [\"pre\", fmt(\"%s\", 1, 2)]\n$b = \"x\" .. sprintf(\"%d\", 1, 2)\n$c = if true then fmt(\"%s\", 1, 2) else \"n\" end\n";
    let diags = lint(src);
    assert_eq!(
        diags,
        vec![
            ("MIX-W2311".to_string(), Some(1)),
            ("MIX-W2311".to_string(), Some(2)),
            ("MIX-W2311".to_string(), Some(3)),
        ]
    );
}

#[test]
fn fmt_surplus_message_carries_expected_and_provided_counts() {
    let src = "print(fmt(\"%s %d\", 1, 2, 3))\n";
    let tokens = Lexer::new(src).tokenize().unwrap();
    let stmts = Parser::new(tokens, src).parse_program().unwrap();
    let a = analyze(&stmts, Some("test.mix"), &AnalyzerConfig::default());
    let d = a
        .diagnostics
        .iter()
        .find(|d| d.code == "MIX-W2311")
        .expect("MIX-W2311 fires");
    assert!(d.message.contains("consumes 2"), "{}", d.message);
    assert!(d.message.contains("3 were provided"), "{}", d.message);
    assert!(
        d.hint
            .as_deref()
            .is_some_and(|h| h.contains("remove") && h.contains("placeholders")),
        "hint names both fixes: {d:?}"
    );
}

#[test]
fn send_status_parameter_default_that_sends_overwrites_the_status() {
    // A default runs when the call omits its argument, so f() may send and
    // overwrite $rc even though its body does not.
    let src = "fn f($x = sh \"true\")\n  return 0\nend\nsend svc ping\nf()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(4)), "got: {got:?}");
}

#[test]
fn send_status_interpolation_fallback_that_sends_overwrites_the_status() {
    // The fallback of ${x ?? ...} runs when x is nil, so its sh overwrites $rc.
    let src = "$x = nil\nsend svc ping\n$s = \"${x ?? sh \"true\"}\"\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(2)), "got: {got:?}");
}

#[test]
fn send_status_builtin_outside_the_pure_allowlist_overwrites_the_status() {
    // read_file does IO, so it is not proven pure and is a barrier.
    let src = "send svc ping\n$t = read_file(\"x.txt\")\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_pure_builtin_keeps_the_send_checked() {
    let src = "send svc ping\n$n = len(\"abc\")\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_pure_parameter_default_keeps_the_call_pure() {
    let src = "fn f($x = len(\"a\"))\n  return 0\nend\nsend svc ping\nf()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(4)), "got: {got:?}");
}

#[test]
fn send_status_pure_interpolation_fallback_keeps_the_send_checked() {
    let src = "$x = nil\nsend svc ping\n$s = \"${x ?? len(\"a\")}\"\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(!got.contains(&Some(2)), "got: {got:?}");
}

#[test]
fn send_status_value_call_overwrites_the_status() {
    let src = "$f = fn() print(\"x\") end\nsend svc ping\n$f()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(2)), "got: {got:?}");
}

#[test]
fn send_status_method_call_overwrites_the_status() {
    let src = "send svc ping\n$m.ping()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_include_overwrites_the_status() {
    let src = "send svc ping\ninclude \"helpers.mix\"\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_call_to_an_unknown_function_overwrites_the_status() {
    // A name defined nowhere in the file is not summarised, so it is a barrier.
    let src = "send svc ping\nhelper_fn()\nif $rc != 0 then exit(1) end\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_deeply_nested_functions_are_collected_once() {
    // Each def used to be walked twice, so N nested defs took 2^N steps. At 40
    // levels that never finishes, so completing is the check.
    let mut src = String::new();
    for depth in 0..40 {
        src.push_str(&format!("fn f{depth}()\n"));
    }
    src.push_str("return 0\n");
    for _ in 0..40 {
        src.push_str("end\n");
    }
    src.push_str("send svc ping\nif $rc != 0 then exit(1) end\n");
    let got = w2307_lines(&src);
    assert!(got.is_empty(), "got: {got:?}");
}

#[test]
fn send_status_call_through_a_callable_parameter_overwrites_the_status() {
    // `helper()` inside `wrapper` runs the callable parameter `$helper`, not
    // the named `helper`, so `wrapper` may write the status.
    let src = "fn helper()\n  return 0\nend\nfn wrapper($helper)\n  helper()\nend\n\
               $f = function() return 1 end\nsend svc ping\nwrapper($f)\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(8)), "got: {got:?}");
}

#[test]
fn send_status_call_through_a_plain_named_function_stays_neutral() {
    // Control for the callable-parameter rule: nothing binds `helper`, so the
    // read after the call still discharges the send.
    let src = "fn helper()\n  return 0\nend\nfn wrapper()\n  helper()\nend\n\
               send svc ping\nwrapper()\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.is_empty(), "got: {got:?}");
}

#[test]
fn send_status_index_is_evaluated_before_the_object_read() {
    // The evaluator runs the index first, so publish() overwrites $result
    // before the object is read.
    let src = "send svc first\n$x = $result[publish(\"events\", \"payload\")]\nprint($x)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(1)), "got: {got:?}");
}

#[test]
fn send_status_read_after_a_halting_call_does_not_discharge_the_send() {
    // `stop` ends the program, directly and through `outer`. The `print($rc)`
    // after the call is unreachable on that path, so it cannot discharge the send.
    let src = "fn stop()\n  exit(0)\nend\nfn outer()\n  stop()\nend\n\
               send svc ping\nouter()\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(7)), "got: {got:?}");
}

#[test]
fn send_status_halting_parameter_default_counts_as_a_halt() {
    // The default always runs in the summary, so `guard` may halt.
    let src = "fn guard($ok = exit(0))\n  return 1\nend\nsend svc ping\nguard()\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(4)), "got: {got:?}");
}

#[test]
fn send_status_return_from_a_function_does_not_end_the_caller() {
    // Control: `return` leaves only the function, so the read after the call
    // still discharges the send.
    let src = "fn early()\n  return 0\nend\nsend svc ping\nearly()\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.is_empty(), "got: {got:?}");
}

#[test]
fn send_status_bare_function_call_statement_applies_its_summary() {
    // `overwrite_status` as a bare statement calls the function, whose send
    // overwrites $rc before the chain's print and the later read.
    let src = "fn overwrite_status()\n  send svc other\nend\n\
               send svc first\noverwrite_status && print(1)\nprint($rc)\n";
    let got = w2307_lines(src);
    assert!(got.contains(&Some(4)), "got: {got:?}");
}
