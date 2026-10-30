// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `mix agent ...` runs agentd's CLI script (<prefix>/lib/agentd/agent_cli.mix)
//! with the same binary, passes the arguments through, and returns the
//! script's own exit code (the CLI uses 2 for "waiting on a human").

use std::process::Command;

fn temp_prefix(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mix-agent-sub-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mix_agent(prefix: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg("agent")
        .args(args)
        // The prefix comes from MIXOS_PREFIX (else the binary's location);
        // MIXOS sets the other roots, not this one.
        .env("MIXOS_PREFIX", prefix)
        .env("MIX_STATS", "off")
        .output()
        .expect("run mix agent")
}

#[test]
fn runs_the_installed_cli_with_its_args_and_exit_code() {
    let prefix = temp_prefix("ok");
    std::fs::create_dir_all(prefix.join("lib/agentd")).unwrap();
    std::fs::write(
        prefix.join("lib/agentd/agent_cli.mix"),
        "print(join(args(), \",\"))\nexit(2)\n",
    )
    .unwrap();
    let out = mix_agent(&prefix, &["send", "abcd1234", "hi there"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "send,abcd1234,hi there\n"
    );
    let _ = std::fs::remove_dir_all(&prefix);
}

#[test]
fn says_so_when_agentd_is_not_installed() {
    let prefix = temp_prefix("missing");
    let out = mix_agent(&prefix, &["list"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    // It must name the temp prefix: a check that only looked for "not
    // installed" passed while MIXOS_PREFIX was being ignored.
    assert!(
        err.contains("not installed") && err.contains(&prefix.display().to_string()),
        "stderr={err}"
    );
    let _ = std::fs::remove_dir_all(&prefix);
}
