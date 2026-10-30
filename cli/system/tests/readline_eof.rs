// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `readline()` at end of input returns nil, and an empty line stays "".
//! Before 2026-10-30 both were "", so a stdin loop could not tell a blank line
//! from EOF and spun forever at EOF. Tested through the real binary's fd 0.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(script: &str, stdin_bytes: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["-c", script])
        .env("MIX_STATS", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mix");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_bytes)
        .expect("write stdin");
    child.wait_with_output().expect("wait for mix")
}

const LOOP: &str = r#"
$n = 0
while true
  $l = readline()
  if $l == nil then break end
  print("[" .. $l .. "]")
  $n = $n + 1
  if $n > 10 then raise("SPIN", "no EOF") end
end
print("eof " .. to_string(readline() == nil))
"#;

#[test]
fn blank_line_is_empty_and_eof_is_nil() {
    let out = run(LOOP, b"a\n\nb\n");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // EOF stays nil on a second read.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "[a]\n[]\n[b]\neof true\n");
}

#[test]
fn last_line_without_newline_is_read_before_eof() {
    let out = run(LOOP, b"a\nlast");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "[a]\n[last]\neof true\n");
}

#[test]
fn empty_input_is_nil_at_once() {
    let out = run(LOOP, b"");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "eof true\n");
}
