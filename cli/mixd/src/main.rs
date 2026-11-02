// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab server host. Re-exec removes the subcommand before existing mains read argv.
use std::{env, ffi::OsStr, os::unix::process::CommandExt, path::Path, process::{Command, ExitCode}};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().collect();
    let name = Path::new(&args[0]).file_name().unwrap_or(OsStr::new("")).to_string_lossy();
    match name.as_ref() {
        "mixd-script" => return mix_shell::main(),
        #[cfg(feature = "noded")]
        "noded" => return noded::main(),
        #[cfg(feature = "maild")]
        "maild" => return maild::main(),
        #[cfg(feature = "webd")]
        "webd" => return webd::main(),
        #[cfg(feature = "dnsd")]
        "dnsd" => return dnsd::main(),
        #[cfg(feature = "monitord")]
        "monitord" => return monitord::main(),
        _ => {}
    }
    let personalities = [
        #[cfg(feature = "noded")] "noded",
        #[cfg(feature = "maild")] "maild",
        #[cfg(feature = "webd")] "webd",
        #[cfg(feature = "dnsd")] "dnsd",
        #[cfg(feature = "monitord")] "monitord",
    ];
    let Some(sub) = args.get(1).and_then(|s| s.to_str()) else {
        eprintln!("usage: mixd run <script.mix> [args] | mixd <personality> [args] | mixd --personalities");
        return ExitCode::from(2);
    };
    if sub == "--personalities" {
        println!("{}", personalities.join("\n"));
        return ExitCode::SUCCESS;
    }
    if (sub == "run" && args.len() > 2) || personalities.contains(&sub) {
        let error = Command::new(env::current_exe().expect("current executable"))
            .arg0(if sub == "run" { "mixd-script" } else { sub })
            .args(&args[2..]).exec();
        eprintln!("mixd: re-exec: {error}");
    } else {
        eprintln!("mixd: unknown personality or missing script: {sub}");
    }
    ExitCode::from(2)
}
