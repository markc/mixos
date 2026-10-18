// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The remote mix binary every remote-mix builtin drives: the `mix_bin`
//! option, else the local `MIX_REMOTE_BIN`, else `/opt/mixos/bin/mix`.
//!
//! A fake `ssh` sits first on PATH. It records its argv (the last word is
//! the remote command string carrying the binary) and runs the shipped
//! source under the mix being tested, so each builtin's real path runs end
//! to end. Out-of-process because the env var is process state.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

const FAKE_SSH: &str = r#"#!/bin/sh
printf '%s\n' "$0" "$@" > "$CAPTURE"
exec "$MIX_UNDER_TEST" -
"#;

struct Bed {
    dir: tempfile::TempDir,
}

impl Bed {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("ssh");
        fs::write(&p, FAKE_SSH).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        Bed { dir }
    }

    fn capture(&self) -> PathBuf {
        self.dir.path().join("ssh-argv")
    }

    /// Run `program` with MIX_REMOTE_BIN set to `env` (or removed).
    fn run(&self, program: &str, env: Option<&str>) -> (String, String, bool) {
        let path = format!("{}:/usr/bin:/bin", self.dir.path().display());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
        cmd.arg("-c")
            .arg(program)
            .env("PATH", path)
            .env("MIX_UNDER_TEST", env!("CARGO_BIN_EXE_mix"))
            .env("CAPTURE", self.capture());
        match env {
            Some(v) => cmd.env("MIX_REMOTE_BIN", v),
            None => cmd.env_remove("MIX_REMOTE_BIN"),
        };
        let out = cmd.output().expect("spawn mix");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.success(),
        )
    }

    /// The remote command string ssh was handed (its last argv word).
    fn remote(&self) -> String {
        let argv = fs::read_to_string(self.capture()).expect("fake ssh ran");
        argv.lines().last().unwrap_or_default().to_string()
    }

    fn ssh_ran(&self) -> bool {
        self.capture().exists()
    }
}

const SSH_MIX: &str = r#"$r = ssh_mix("alpha", "print(1+1)"OPTS)
print($r.ok .. " " .. trim($r.stdout))
"#;

fn ssh_mix(opts: &str) -> String {
    SSH_MIX.replace("OPTS", opts)
}

#[test]
fn ssh_mix_defaults_to_opt_mixos() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(&ssh_mix(""), None);
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(out.trim(), "true 2", "stderr={err}");
    assert_eq!(bed.remote(), "/opt/mixos/bin/mix -");
}

#[test]
fn ssh_mix_honours_the_mix_bin_option() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(&ssh_mix(r#", {mix_bin: "/usr/local/bin/mix"}"#), None);
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(out.trim(), "true 2", "stderr={err}");
    assert_eq!(bed.remote(), "/usr/local/bin/mix -");
}

#[test]
fn ssh_mix_honours_mix_remote_bin_from_the_environment() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(&ssh_mix(""), Some("/usr/bin/mix"));
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(out.trim(), "true 2", "stderr={err}");
    assert_eq!(bed.remote(), "/usr/bin/mix -");
}

#[test]
fn the_option_beats_the_environment() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(
        &ssh_mix(r#", {mix_bin: "/usr/local/bin/mix", strict_arity: true}"#),
        Some("/usr/bin/mix"),
    );
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(bed.remote(), "/usr/local/bin/mix --strict-arity -");
}

#[test]
fn an_empty_environment_value_means_the_default() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(&ssh_mix(""), Some(""));
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(bed.remote(), "/opt/mixos/bin/mix -");
}

#[test]
fn a_bad_value_raises_locally_and_never_reaches_ssh() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(&ssh_mix(r#", {mix_bin: "mix; rm"}"#), None);
    assert!(!ok, "stdout={out} stderr={err}");
    assert!(err.contains("mix_bin"), "{err}");
    assert!(!bed.ssh_ran(), "ssh must not run on a bad mix_bin");

    let (out, err, ok) = bed.run(&ssh_mix(""), Some("mix; rm"));
    assert!(!ok, "stdout={out} stderr={err}");
    assert!(err.contains("MIX_REMOTE_BIN"), "{err}");
    assert!(!bed.ssh_ran(), "ssh must not run on a bad MIX_REMOTE_BIN");
}

#[test]
fn ssh_mix_many_inherits_the_option() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(
        r#"$r = ssh_mix_many(["alpha"], "print(3)", {mix_bin: "/usr/local/bin/mix"})
print($r.alpha.ok .. " " .. trim($r.alpha.stdout))
"#,
        None,
    );
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(out.trim(), "true 3", "stderr={err}");
    assert_eq!(bed.remote(), "/usr/local/bin/mix -");
}

#[test]
fn ssh_run_env_driver_uses_the_resolved_binary() {
    let bed = Bed::new();
    let (out, err, ok) = bed.run(
        r#"$r = ssh_run("alpha", "echo $K", {env: {K: "v"}})
print($r.ok .. " " .. trim($r.stdout))
"#,
        Some("/usr/local/bin/mix"),
    );
    assert!(ok, "stdout={out} stderr={err}");
    assert_eq!(out.trim(), "true v", "stderr={err}");
    assert_eq!(bed.remote(), "/usr/local/bin/mix -");
}

#[test]
fn ssh_exec_takes_mix_bin_and_the_old_remote_mix_name() {
    for key in ["mix_bin", "remote_mix"] {
        let bed = Bed::new();
        let (out, err, ok) = bed.run(
            &format!(
                "$r = ssh_exec(\"alpha\", [\"/bin/echo\", \"hi\"], {{{key}: \"/usr/local/bin/mix\"}})\nprint($r.ok .. \" \" .. trim($r.stdout))\n"
            ),
            None,
        );
        assert!(ok, "{key}: stdout={out} stderr={err}");
        assert_eq!(out.trim(), "true hi", "{key}: stderr={err}");
        assert_eq!(bed.remote(), "/usr/local/bin/mix -", "{key}");
    }
}
