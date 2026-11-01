// SPDX-License-Identifier: MIT OR Apache-2.0
use mix::{evaluator::{Evaluator, SharedBuf}, lexer::Lexer, parser::Parser};

async fn run(source: &str) -> String {
    let stmts = Parser::new(Lexer::new(source).tokenize().unwrap(), source).parse_program().unwrap();
    let out = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(out.clone()), Box::new(SharedBuf::new()));
    eval.execute(&stmts).await.unwrap();
    out.to_string_lossy().trim_end().to_string()
}

#[tokio::test]
async fn helper_writes_and_fast_paths() {
    assert_eq!(run(r#"
$conns = {a: 1, b: 2}
$n = 0
$s = ""
$m = {a: {b: 0}, global: 1}
$xs = [0]
fn update()
  global $conns, $n, $s, $m, $xs, $created
  $conns = delete($conns, "a")
  $n = $n + 1
  $s = $s .. "x"
  $m.a.b = $n
  $m.global = $m["global"] + 1
  $xs[0] = $n
  push($xs, 2)
  $created = 42
  for $i = 1 to 5
    $n = $n + 1
  end
end
update()
update()
print(length($conns))
print($n)
print($s)
print($m.a.b)
print($m.global)
print($xs[0])
print($created)
"#).await, "1\n12\nxx\n7\n3\n7\n42");
}

#[tokio::test]
async fn declaration_errors_are_catchable() {
    for (body, code) in [
        ("fn f($x) global $x end; f(1)", "GLOBAL_PARAM"),
        ("fn f() $x = 1; global $x end; f()", "GLOBAL_AFTER_LOCAL"),
        ("fn f() global $x; for $x = 1 to 0 end end; f()", "GLOBAL_BINDER"),
        ("fn f() global $x; for $x in [] end end; f()", "GLOBAL_BINDER"),
        ("fn f() global $x; for each $x in [] end end; f()", "GLOBAL_BINDER"),
        ("fn f() global $x; parse \"a\" with $x end; f()", "GLOBAL_BINDER"),
        ("fn f() global $x; try die(\"e\") catch $x end end; f()", "GLOBAL_BINDER"),
        ("fn f() global $x; try die(\"e\") catch $m, $x end end; f()", "GLOBAL_BINDER"),
        ("fn f() global $event end; f()", "GLOBAL_RUNTIME_NAME"),
        ("global $rc", "GLOBAL_RUNTIME_NAME"),
        ("global $result", "GLOBAL_RUNTIME_NAME"),
        ("global $reply", "GLOBAL_RUNTIME_NAME"),
    ] {
        let source = format!("try {body} catch $msg, $err print($err.code) end");
        assert_eq!(run(&source).await, code, "{body}");
    }
}

#[tokio::test]
async fn invocation_isolation_recursion_and_lambdas() {
    assert_eq!(run(r#"
$n = 0
fn local_only() $n = 100 end
fn recurse($depth)
  if $depth > 0 then
    global $n
    global $n
    $n = $n + 1
    local_only()
    recurse($depth - 1)
  else
    $n = 99
  end
end
recurse(4)
print($n)
fn make()
  global $n
  $f = fn() $n = 200 end
  $f()
  $g = fn() global $n; $n = $n + 1 end
  $g()
end
make()
print($n)
fn captured()
  $n = 10
  $f = fn() global $n; return $n end
  $f()
end
try captured() catch $m, $e print($e.code) end
"#).await, "4\n5\nGLOBAL_AFTER_LOCAL");
}

#[tokio::test]
async fn loop_rhs_function_cannot_rehash_under_raw_slots() {
    assert_eq!(run(r#"
$acc = 0
fn inserting($i)
  global $new_binding
  $new_binding = $i
  return $i
end
for $i = 1 to 200
  $acc = $acc + inserting($i)
end
print($acc)
print($new_binding)
"#).await, "20100\n200");
}

#[tokio::test]
async fn missing_global_read_is_undefined_and_keyword_keys_work() {
    assert_eq!(run("global $a; $a = {global: 1}; print($a.global); print($a[\"global\"]); fn f() global $missing; return $missing end; try f() catch $m, $e print($e.code) end").await,
        "1\n1\nNAME_UNDEFINED");
}
