// SPDX-License-Identifier: MIT OR Apache-2.0
//! `script_path()` — the entry script's `$0`, resolved the way `realpath($0)`
//! resolves it. The CLI sets `$0` to the path the script was run as; a
//! required module must still see the ENTRY script, not itself.

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;
use mix::value::Value;
use std::path::{Path, PathBuf};

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mix_script_path_test_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `main_src` as the entry script, the way the CLI does: the file
/// context is set to `main_path`, and `$0` is set to `dollar_zero` when
/// given (`None` leaves it unset, as `mix -c` and the REPL do).
async fn run_entry(
    main_path: &Path,
    main_src: &str,
    dollar_zero: Option<&str>,
) -> Result<String, String> {
    let mut lexer = Lexer::new(main_src);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, main_src);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    if let Some(zero) = dollar_zero {
        eval.set_global("0", Value::String(zero.to_string()));
        // The CLI records the entry identity here, before any script code.
        eval.set_entry_script(zero);
    }
    eval.set_file(main_path.to_string_lossy().to_string());
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

fn canonical_line(path: &Path) -> String {
    format!("{}\n", std::fs::canonicalize(path).unwrap().display())
}

#[tokio::test]
async fn script_path_is_the_resolved_dollar_zero() {
    let dir = test_dir("entry");
    let main = dir.join("main.mix");
    std::fs::write(&main, "").unwrap();
    let zero = main.to_string_lossy().to_string();
    let out = run_entry(&main, "print(script_path())\n", Some(&zero))
        .await
        .unwrap();
    assert_eq!(out, canonical_line(&main));
}

#[cfg(unix)]
#[tokio::test]
async fn script_path_resolves_symlinks_like_realpath() {
    // `$0` names the symlink; realpath($0) and script_path() both give the
    // file it points at, so a script finds its siblings from the real tree.
    let dir = test_dir("symlink");
    let real = dir.join("real.mix");
    std::fs::write(&real, "").unwrap();
    let link = dir.join("link.mix");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let zero = link.to_string_lossy().to_string();
    let src = "print(script_path())\nprint(realpath($0))\n";
    let out = run_entry(&link, src, Some(&zero)).await.unwrap();
    let expected = canonical_line(&real);
    assert_eq!(out, format!("{expected}{expected}"));
}

#[tokio::test]
async fn required_module_sees_the_entry_script_not_itself() {
    let dir = test_dir("module");
    std::fs::write(
        dir.join("lib.mix"),
        "fn where_am_i()\n  return script_path()\nend\n",
    )
    .unwrap();
    let main = dir.join("main.mix");
    std::fs::write(&main, "").unwrap();
    let zero = main.to_string_lossy().to_string();
    let src = "$m = require(\"lib.mix\")\nprint($m.where_am_i())\nprint(script_path())\n";
    let out = run_entry(&main, src, Some(&zero)).await.unwrap();
    let expected = canonical_line(&main);
    assert_eq!(out, format!("{expected}{expected}"));
}

#[tokio::test]
async fn script_path_in_a_module_top_level_init_is_the_entry() {
    // The module's top level runs while exec_require has swapped its own
    // scope in. The answer must still be the entry script, not the module.
    let dir = test_dir("module_init");
    std::fs::write(dir.join("lib.mix"), "return {path: script_path()}\n").unwrap();
    let main = dir.join("main.mix");
    std::fs::write(&main, "").unwrap();
    let zero = main.to_string_lossy().to_string();
    let src = "$m = require(\"lib.mix\")\nprint($m.path)\n";
    let out = run_entry(&main, src, Some(&zero)).await.unwrap();
    assert_eq!(out, canonical_line(&main));
}

#[tokio::test]
async fn script_path_through_a_nested_require_is_the_entry() {
    // Both the inner module's init and the outer module's init must see
    // the entry script; the outer module reports its own call and the inner
    // module's answer.
    let dir = test_dir("nested_require");
    std::fs::write(dir.join("inner.mix"), "return {path: script_path()}\n").unwrap();
    std::fs::write(
        dir.join("outer.mix"),
        "$i = require(\"inner.mix\")\nreturn {path: script_path(), inner: $i.path}\n",
    )
    .unwrap();
    let main = dir.join("main.mix");
    std::fs::write(&main, "").unwrap();
    let zero = main.to_string_lossy().to_string();
    let src = "$o = require(\"outer.mix\")\nprint($o.path)\nprint($o.inner)\n";
    let out = run_entry(&main, src, Some(&zero)).await.unwrap();
    let expected = canonical_line(&main);
    assert_eq!(out, format!("{expected}{expected}"));
}

#[tokio::test]
async fn script_path_is_nil_without_a_script_file() {
    // `mix -c`, the REPL: $0 is never set.
    let dir = test_dir("unset");
    let main = dir.join("main.mix");
    let out = run_entry(&main, "print(script_path())\n", None).await.unwrap();
    assert_eq!(out, "nil\n");
}

#[tokio::test]
async fn script_path_is_nil_for_stdin() {
    // `mix -`: $0 is the dash, not a file.
    let dir = test_dir("stdin");
    let main = dir.join("main.mix");
    let out = run_entry(&main, "print(script_path())\n", Some("-"))
        .await
        .unwrap();
    assert_eq!(out, "nil\n");
}

#[tokio::test]
async fn script_path_is_nil_when_the_file_has_gone() {
    // realpath($0) answers nil for a missing component, and so does
    // script_path() when the path cannot be resolved at start.
    let dir = test_dir("gone");
    let main = dir.join("main.mix");
    let gone = dir.join("no-such-dir").join("gone.mix");
    let zero = gone.to_string_lossy().to_string();
    let out = run_entry(&main, "print(script_path())\n", Some(&zero))
        .await
        .unwrap();
    assert_eq!(out, "nil\n");
}
