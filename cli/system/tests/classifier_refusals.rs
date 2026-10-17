// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The shell classifier's refusal layer (B1, B3, 09-29 #1): a bash
//! keyword written as a shell command fails ONCE and legibly instead of
//! piecemeal at exit 0, an unquoted newline is a `;`, and a misrouted
//! Mix one-liner names itself.

use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(args)
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .output()
        .expect("run mix");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn unquoted_newline_is_a_separator() {
    // B1: `mix -c $'true\nfalse'` must run BOTH commands — the second
    // one's exit code wins (1), never a single command with a newline in
    // its filename at exit 0.
    let (code, _, _) = run(&["-c", "true\nfalse"]);
    assert_eq!(code, 1, "a bare newline must act like ';'");
}

#[test]
fn bash_keyword_refuses_the_whole_list() {
    let (code, out, err) = run(&["-c", "for x in 1; do echo hi; done; echo END"]);
    assert_eq!(code, 2, "a bash keyword head must refuse the whole list");
    assert!(!out.contains("END"), "the chain must not carry on: {out}");
    assert!(
        err.contains("`for` is bash"),
        "the refusal must name the Mix form: {err}"
    );
}

#[test]
fn dollar_question_marks_are_refused_with_the_rc_pointer() {
    let (code, _, err) = run(&["-c", "echo $?"]);
    assert_eq!(code, 2, "a bare $? must be refused");
    assert!(err.contains("$rc"), "the refusal must point at $rc: {err}");
}

#[test]
fn double_dollar_is_the_shell_pid() {
    // TODO-mix 2026-10-09: `$$` reached the program as two dollar signs, so
    // `ps -p $$` failed with the child's confusing error. It is now the
    // shell's PID, as in bash and zsh — the mix process running the line.
    let child = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["-c", "echo pid=$$"])
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run mix");
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert_eq!(stdout.trim(), format!("pid={pid}"));
    // Single-quoted, it stays literal.
    let (code, out, _) = run(&["-c", "echo '$$'"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "$$");
}

#[test]
fn bash_special_parameters_are_refused_with_their_mix_spelling() {
    for (line, pointer) in [
        ("echo $!", "spawn("),
        ("echo $#", "len(args())"),
        ("echo $@", "args()"),
        ("echo $*", "args()"),
    ] {
        let (code, _, err) = run(&["-c", line]);
        assert_eq!(code, 2, "`{line}` must be refused: {err}");
        assert!(err.contains(pointer), "`{line}` must point at {pointer}: {err}");
    }
    let (code, out, _) = run(&["-c", "echo '$# $@ $! $*'"]);
    assert_eq!(code, 0, "single-quoted specials are literal");
    assert_eq!(out.trim(), "$# $@ $! $*");
    // Inside `$(...)` they are the inner shell's own.
    let (code, out, err) = run(&["-c", "echo n=$(sh -c 'echo $#' x a b) $(printf '%s' $#)"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "n=2 0");
}

#[test]
fn a_mix_variable_reaches_a_command_substitution() {
    // TODO-mix 2026-10-10: `$(...)` handed its text to /bin/sh unchanged, so
    // a Mix local expanded to "" without a word.
    for (src, want) in [
        ("$d = \"x\"\nprint($(echo $d))", "x"),
        ("$d = \"x\"\nprint($(echo pre-$d-post))", "pre-x-post"),
        ("$d = \"x\"\nprint($(sh -c \"echo $d\"))", "x"),
        ("$d = \"; echo injected\"\nprint($(echo $d))", "; echo injected"),
        ("print($(for f in a b; do echo $f; done))", "a\nb"),
    ] {
        let (code, out, err) = run(&["--no-lint", "-c", src]);
        assert_eq!(code, 0, "{src}: {err}");
        assert_eq!(out.trim_end(), want, "{src}");
    }
    let (code, _, err) = run(&["--no-lint", "-c", "print($(echo $zz_no_such_var))"]);
    assert_ne!(code, 0, "an unset name must raise");
    assert!(err.contains("`$zz_no_such_var`"), "{err}");
    // The inner shell's own text is left alone: a quoted heredoc body, a
    // comment, an unused default operand, an argument shaped like an
    // assignment.
    for (src, want) in [
        (
            "print($(cat <<'EOF'\n$zz_no_such_var\nEOF\n))",
            "$zz_no_such_var",
        ),
        ("print($(echo hi # $zz_no_such_var\n))", "hi"),
        ("$o = \"ok\"\nprint($(echo ${o:-$zz_no_such_var}))", "ok"),
        ("$zz = \"outer\"\nprint($(echo zz=inner $zz))", "zz=inner outer"),
    ] {
        let (code, out, err) = run(&["--no-lint", "-c", src]);
        assert_eq!(code, 0, "{src}: {err}");
        assert_eq!(out.trim_end(), want, "{src}");
    }
}

#[test]
fn a_command_line_substitution_sees_the_shells_variables_too() {
    // The shell-command path resolves through the same rule. An environment
    // variable still works; an unset name refuses the command.
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["-c", "echo got-$(echo $ZZ_SUBST_PROBE)"])
        .env("ZZ_SUBST_PROBE", "env-value")
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .output()
        .expect("run mix");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "got-env-value");
    let (code, out, err) = run(&["-c", "echo got-$(echo $zz_no_such_var)"]);
    assert_ne!(code, 0, "an unset name must refuse the command: {out}");
    assert!(!out.contains("got-"), "the command must not run: {out}");
    assert!(err.contains("is not set"), "{err}");
}

#[test]
fn single_quoted_dollar_question_stays_literal() {
    // `'$?'` is literal text in sh semantics — it must keep working.
    let (code, out, _) = run(&["-c", "echo '$?'"]);
    assert_eq!(code, 0, "a single-quoted $? is literal, not a refusal");
    assert!(out.contains("$?"), "the literal must reach echo: {out}");
}

#[test]
fn keyword_word_as_an_argument_is_not_a_head() {
    // `echo for` — 'for' is an ARGUMENT, the head is echo.
    let (code, out, _) = run(&["-c", "echo for"]);
    assert_eq!(code, 0);
    assert!(out.contains("for"));
}

#[test]
fn misrouted_mix_line_names_itself() {
    let (code, _, err) = run(&["-c", "nosuchone\nnosuchtwo"]);
    assert_ne!(code, 0);
    assert!(
        err.contains("look like Mix keywords"),
        "all-ENOENT lines must say so: {err}"
    );
}

#[test]
fn a_single_typo_stays_a_plain_enoent() {
    // One ENOENT piece is a typo, not a misrouted program — no heuristic.
    let (code, _, err) = run(&["-c", "typo-command-xyz"]);
    assert_ne!(code, 0);
    assert!(
        !err.contains("look like Mix keywords"),
        "a single typo must not trigger the heuristic: {err}"
    );
}
