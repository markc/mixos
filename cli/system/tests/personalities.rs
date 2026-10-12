// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every personality of the system binary reports its own component and
//! version, never the host's: argv[0] picks the program, and build
//! provenance is captured inside each component's library.

use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_mix");

/// A fresh directory holding symlinks to the binary, one per name.
fn links(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in names {
        symlink(BIN, dir.path().join(name)).unwrap();
    }
    dir
}

fn run(command: &mut Command) -> (i32, String, String) {
    let out = command.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_shell_identity(command: &mut Command) {
    let (code, out, err) = run(command.args(["--version", "--json"]));
    assert_eq!(code, 0, "stderr: {err}");
    let doc: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["component"], "mix");
    assert_eq!(doc["version"], mix_shell::COMPONENT_VERSION);
}

#[test]
fn every_personality_reports_its_own_identity() {
    let dir = links(&["mix", "noded"]);

    let (code, out, err) = run(Command::new(dir.path().join("mix")).arg("--version"));
    assert_eq!(code, 0, "stderr: {err}");
    let want = format!("mix {} (", mix_shell::COMPONENT_VERSION);
    assert!(out.starts_with(&want), "got {out:?}, want prefix {want:?}");
    assert_shell_identity(&mut Command::new(dir.path().join("mix")));

    let (code, out, err) = run(Command::new(dir.path().join("noded")).arg("--version"));
    assert_eq!(code, 0, "stderr: {err}");
    let want = format!("noded {} (", noded::COMPONENT_VERSION);
    assert!(out.starts_with(&want), "got {out:?}, want prefix {want:?}");
}

#[test]
fn the_file_itself_is_the_shell() {
    assert_shell_identity(&mut Command::new(BIN));
}

#[test]
fn a_login_shell_is_the_shell() {
    assert_shell_identity(Command::new(BIN).arg0("-mix"));
}

#[test]
fn an_unset_self_exec_name_is_the_shell() {
    assert_shell_identity(Command::new(BIN).arg0("/proc/self/exe"));
}

#[test]
fn personalities_lists_every_component() {
    let (code, out, _) = run(Command::new(BIN).arg("--personalities"));
    assert_eq!(code, 0);
    let want = format!(
        "mix {}\nnoded {}\n",
        mix_shell::COMPONENT_VERSION,
        noded::COMPONENT_VERSION
    );
    assert_eq!(out, want);
}

#[test]
fn an_unknown_name_exits_2_with_the_list() {
    let dir = links(&["maild"]);
    let (code, out, err) = run(&mut Command::new(dir.path().join("maild")));
    assert_eq!(code, 2);
    assert_eq!(out, "");
    assert!(err.starts_with("maild: not a personality"), "got {err:?}");
    assert!(
        err.contains(&format!("noded {}", noded::COMPONENT_VERSION)),
        "got {err:?}"
    );
}
