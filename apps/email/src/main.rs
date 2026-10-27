// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The entry point: parse, hand an already-running window the activation,
//! or run the window (lib.rs).
const HELP: &str = "email — the MixOS JMAP mail, calendar and contacts client
Usage: email [--noded-url URL] [--service NAME]";

fn main() {
    let mut cli_args: Vec<String> = std::env::args().collect();
    let _logging = init_logging(&mut cli_args);
    buildinfo::exit_on_version!(leading);
    if matches!(cli_args.get(1).map(String::as_str), Some("--help" | "-h")) {
        println!("{HELP}\n{}", toolkit::debug::HELP);
        return;
    }
    let args = match email::parse(cli_args.into_iter().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            toolkit::debug::failure("email", &error);
            eprintln!("email: {error}");
            std::process::exit(2);
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
        toolkit::debug::failure("email", &error);
        eprintln!("email: {error}");
        std::process::exit(1);
    }
}

fn init_logging(args: &mut Vec<String>) -> Option<logging::LogHandle> {
    match toolkit::debug::init(email::APP, "warn,email=info", args) {
        Ok(handle) => Some(handle),
        Err(error) => {
            toolkit::debug::failure("email", &error);
            eprintln!("email: {error}");
            // Retain Email's non-fatal policy for other logging failures.
            // Malformed environment filters recover inside the shared helper.
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_log_filter_child() {
        if std::env::var_os("EMAIL_TEST_INVALID_LOG").is_some() {
            assert!(super::init_logging(&mut vec!["email".into()]).is_some());
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
