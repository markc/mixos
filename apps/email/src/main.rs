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
    let _logging = init_logging();
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

fn init_logging() -> Option<logging::LogHandle> {
    match logging::init(
        &logging::LogOpts::default(),
        &logging::StatsOpts::default(),
        logging::LogDefaults {
            default_filter: Some("warn,email=info".into()),
            ..logging::LogDefaults::gui(email::APP)
        },
    ) {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("email: {error}");
            // Logging must never prevent the GUI from starting. Continue
            // without a subscriber if the environment filter is malformed.
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_log_filter_child() {
        if std::env::var_os("EMAIL_TEST_INVALID_LOG").is_some() {
            assert!(super::init_logging().is_none());
            println!("startup continues");
        }
    }

    #[test]
    fn invalid_log_filter_does_not_abort_startup() {
        // Isolate the global subscriber and environment from other tests.
        // This exercises logging only; it never opens a window or the Bus.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::invalid_log_filter_child", "--nocapture"])
            .env("EMAIL_TEST_INVALID_LOG", "1")
            .env("RUST_LOG", "[invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("startup continues"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("email:"));
    }
}
