// SPDX-License-Identifier: MIT OR Apache-2.0
use std::{os::unix::process::CommandExt, process::Command};

#[test]
fn run_listens_and_preserves_script_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("listen.mix");
    std::fs::write(&path, "$l = tcp_listen(\"127.0.0.1\", 0)\nprint(tcp_local_addr($l).port > 0)\nprint(args()[0])\ntcp_close($l)\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mixd"))
        .arg("run").arg(path).arg("sentinel").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true\nsentinel\n");
}

#[test]
fn personalities_dispatch_by_subcommand_and_argv0() {
    let bin = env!("CARGO_BIN_EXE_mixd");
    let listed = Command::new(bin).arg("--personalities").output().unwrap();
    assert!(listed.status.success());
    let names = String::from_utf8_lossy(&listed.stdout);
    for name in ["noded", "maild", "webd", "dnsd", "monitord"] {
        assert!(names.lines().any(|n| n == name));
        for out in [Command::new(bin).args([name, "--version", "--json"]).output().unwrap(),
            Command::new(bin).arg0(name).args(["--version", "--json"]).output().unwrap()] {
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(doc["component"], name);
        }
    }
}
