// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The entry point: parse, hand an already-running window the activation,
//! or run the window (lib.rs).
const HELP: &str = "email — the MixOS JMAP mail, calendar and contacts client
Usage: email [--noded-url URL] [--service NAME]";

fn main() {
    buildinfo::exit_on_version!(leading);
    if matches!(std::env::args().nth(1).as_deref(), Some("--help" | "-h")) {
        println!("{HELP}");
        return;
    }
    let args = match email::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("email: {error}");
            std::process::exit(2);
        }
    };
    let _logging = match logging::init(
        &logging::LogOpts::default(),
        &logging::StatsOpts::default(),
        logging::LogDefaults {
            default_filter: Some("warn,email=info".into()),
            ..logging::LogDefaults::gui(email::APP)
        },
    ) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("email: {error}");
            std::process::exit(1);
        }
    };
    let result = if citizen::probe(&args.url, &args.service, email::APP) {
        // One window per session: a second launch raises the first.
        citizen::forward(&args.url, &args.service, email::APP)
    } else {
        match email::run(&args) {
            // Another launch won the race to register: hand it ours.
            Err(_) if citizen::probe(&args.url, &args.service, email::APP) => {
                citizen::forward(&args.url, &args.service, email::APP)
            }
            result => result,
        }
    };
    if let Err(error) = result {
        eprintln!("email: {error}");
        std::process::exit(1);
    }
}
