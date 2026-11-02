// SPDX-License-Identifier: MIT OR Apache-2.0
use std::process::Command;

#[test]
fn server_calls_have_metadata_runtime_refusals_and_lint_errors() {
    for (name, args) in [("tcp_listen", "\"127.0.0.1\", 0"), ("tcp_accept", "1"),
        ("udp_listen", "\"127.0.0.1\", 0"), ("http_serve", "\".\"")] {
        let out = Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(["--no-lint", "-c", &format!("try; {name}({args}); catch $m, $e; print($e.code .. \" \" .. $m); end")]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{name}: {:?}", out);
        assert!(err.contains("NEEDS_MIXD") && err.contains(name) && err.contains("mixd run"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.mix");
        std::fs::write(&path, format!("{name}({args})\n")).unwrap();
        let lint = Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(["lint", "--json"]).arg(&path).output().unwrap();
        assert!(!lint.status.success());
        assert!(String::from_utf8_lossy(&lint.stdout).contains("MIX-E1513"), "{:?}", lint);
    }
}
