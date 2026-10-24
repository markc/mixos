// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `mix lint` CLI pins for 0.117.0: `--fix` applies the bareword-null quick
//! fix, `--json` reports it, `-- lint:` pragmas let a file lint clean on its
//! own, and the binary's shell extensions are known callables.

use std::process::Command;

fn write_temp(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mix_lint_fix_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.mix"));
    std::fs::write(&path, source).unwrap();
    path
}

fn lint(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg("lint")
        .args(args)
        .env("MIX_STATS", "off")
        .output()
        .expect("run mix lint");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn fix_rewrites_bareword_null_to_nil() {
    let path = write_temp(
        "null",
        "$x = {open: null, b: None}\nprint(json_encode($x))\n",
    );
    let p = path.to_str().unwrap();
    let (code, out, _) = lint(&["--deny-warnings", p]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("MIX-W2312"), "{out}");
    assert!(out.contains("fix: replace `null` with `nil`"), "{out}");

    let (code, out, err) = lint(&["--deny-warnings", "--fix", p]);
    assert_eq!(code, 0, "the fixed file lints clean: {out}");
    assert!(err.contains("applied 2 fix(es)"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "$x = {open: nil, b: nil}\nprint(json_encode($x))\n"
    );
}

#[test]
fn json_reports_the_fix() {
    let path = write_temp("json", "$x = null\nprint($x)\n");
    let (_, out, _) = lint(&["--json", path.to_str().unwrap()]);
    let report: serde_json::Value = serde_json::from_str(&out).expect("json");
    let d = &report["diagnostics"][0];
    assert_eq!(d["code"], "MIX-W2312");
    assert_eq!(d["fix"]["replace"], "null");
    assert_eq!(d["fix"]["with"], "nil");
    assert_eq!(d["fix"]["line"], 1);
    assert_eq!(d["fix"]["column"], 6);
}

#[test]
fn pragmas_make_an_injected_body_and_a_generator_lint_clean() {
    // The two operator-script shapes: a remote body whose caller
    // binds $sha, and a generator whose heredoc is literal Mix source.
    let body = write_temp(
        "body",
        "-- version: 0.1.0\n-- lint: globals sha\nprint(slice($sha, 0, 12))\n",
    );
    let generator = write_temp(
        "generator",
        "-- version: 0.1.0\n$pid = 1\n-- lint: ignore MIX-W2402\nwrite_file(\"/tmp/x.mix\", <<EOF\n$pid = spawn([\"true\"])\nEOF\n)\nprint($pid)\n",
    );
    let (code, out, _) = lint(&[
        "--deny-warnings",
        body.to_str().unwrap(),
        generator.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("0 error(s), 0 warning(s), 0 note(s)"), "{out}");
}

#[test]
fn the_w2307_reproducer_lints_clean_through_the_driver() {
    // TODO-mix 2026-10-06, verbatim: the status is read in the condition and
    // in a map value nested in a call argument. A truly unchecked send and a
    // read that only follows the NEXT send still warn.
    let checked = write_temp(
        "w2307",
        "$target=\"svc\"\n$command=\"ping\"\n$payload=\"{}\"\nsend $target $command body=$payload timeout=45\nif $rc != 0 then exit(1) end\nprint(json_encode({rc:$rc,result:$result}))\n",
    );
    let (code, out, _) = lint(&["--deny-warnings", checked.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("MIX-W2307"), "{out}");
    let unchecked = write_temp(
        "w2307_unchecked",
        "send svc ping\nsend svc ping\nif $rc != 0 then exit(1) end\n",
    );
    let (_, out, _) = lint(&[unchecked.to_str().unwrap()]);
    assert_eq!(out.matches("MIX-W2307").count(), 1, "{out}");
    assert!(out.contains(":1: MIX-W2307"), "the first send: {out}");
}

#[test]
fn shell_extensions_are_known_and_shadowing_one_warns() {
    let path = write_temp(
        "ext",
        "fn context($over)\n  return 1\nend\nprint(ai(\"q\"))\n",
    );
    let (code, out, _) = lint(&[path.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("MIX-W2313"), "{out}");
    assert!(!out.contains("MIX-E1102"), "{out}");
}
