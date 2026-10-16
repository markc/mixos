// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The system binary, `mix`: the shell and the headless daemons in one file
//! (docs/docs/decisions/2026-10-12-one-system-binary.md). The personality comes
//! from argv[0]'s basename, so each daemon is a symlink to `mix` named after
//! its component and still runs as its own process and unit. Nothing else
//! lives here: each component's program is its own library's `main`, which
//! sets up its own runtime, logging identity and build provenance.

use std::ffi::OsStr;
use std::path::Path;
use std::process::ExitCode;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Every personality this binary carries, with its component's version.
const PERSONALITIES: &[(&str, &str)] = &[
    ("mix", mix_shell::COMPONENT_VERSION),
    ("noded", noded::COMPONENT_VERSION),
    ("monitord", monitord::COMPONENT_VERSION),
];

/// The personality argv[0] names: its basename, without the leading `-` a
/// login shell is started with (`-mix`).
fn personality_name(argv0: &OsStr) -> String {
    let base = Path::new(argv0)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match base.strip_prefix('-') {
        Some(login) => login.to_owned(),
        None => base,
    }
}

/// This file's own name answers as the shell, whatever it is called (a
/// development copy, say).
fn is_own_file(name: &str) -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.file_name().is_some_and(|own| own == name))
}

fn list() -> String {
    PERSONALITIES
        .iter()
        .map(|(name, version)| format!("{name} {version}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let name = args
        .next()
        .map(|argv0| personality_name(&argv0))
        .unwrap_or_default();
    if args.next().is_some_and(|first| first == "--personalities") {
        println!("{}", list());
        return ExitCode::SUCCESS;
    }
    match name.as_str() {
        // `exe`: a `/proc/self/exe` re-exec that did not set argv[0].
        "mix" | "exe" => mix_shell::main(),
        "noded" => noded::main(),
        "monitord" => monitord::main(),
        own if is_own_file(own) => mix_shell::main(),
        _ => {
            eprintln!(
                "{name}: not a personality of this binary. It carries:\n{}",
                list()
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::personality_name;
    use std::ffi::OsStr;

    #[test]
    fn the_name_is_the_basename_without_a_login_dash() {
        assert_eq!(
            personality_name(OsStr::new("/opt/mixos/bin/noded")),
            "noded"
        );
        assert_eq!(personality_name(OsStr::new("-mix")), "mix");
        assert_eq!(personality_name(OsStr::new("mix")), "mix");
        assert_eq!(personality_name(OsStr::new("/proc/self/exe")), "exe");
        assert_eq!(personality_name(OsStr::new("")), "");
    }
}
