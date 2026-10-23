// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `screenshot [--capture MODE [--window ID:GEN]] [IMAGE]`: one window per
//! compositor ([`screenshot::names`]). A second launch hands its request to
//! the running window of the same compositor
//! (`screenshot.take`, or `screenshot.open` then `screenshot.show`) and
//! exits; Print Screen launches it this way. `--headless` serves the same
//! verbs with no window, for agents.
use screenshot::capture::{self, Mode, Target};
use screenshot::shell::{Shell, Start};
use screenshot::{APP_ID, SERVICE, label, names};
use serde_json::json;
use std::path::PathBuf;

const HELP: &str = "screenshot — native screenshots and editable annotations\n\
Usage: screenshot [OPTIONS] [IMAGE]\n\
  --capture MODE     take a screenshot at once: region, screen or window\n\
                     (as Print Screen does); a running window takes it\n\
  --window ID:GEN    the window to capture (with --capture window)\n\
  --headless         a Bus service with no window, for agents\n\
                     (screenshot-headless, per compositor)\n\
  --service NAME     register as NAME instead (tests)\n\
  --comp NAME        the compositor service (default comp)\n\
  --noded-url URL    the broker (default: the session's)\n\
  --version          version and exact build provenance\n\
Saved shots go to Pictures/Screenshots; Bus: screenshot.* (screenshot.v1).";

#[derive(Debug)]
struct Args {
    headless: bool,
    service: String,
    comp: String,
    url: String,
    path: Option<PathBuf>,
    capture: Option<Mode>,
    window: Option<Target>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        headless: false,
        service: SERVICE.into(),
        comp: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
        url: citizen::noded_url(),
        path: None,
        capture: None,
        window: None,
    };
    let mut args = args;
    let mut positional = false;
    let mut service_given = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !positional => positional = true,
            "--headless" if !positional => out.headless = true,
            "--service" | "--comp" | "--noded-url" | "--capture" | "--window" if !positional => {
                let value = args
                    .next()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("{arg} needs a value"))?;
                match arg.as_str() {
                    "--service" => {
                        out.service = value;
                        service_given = true;
                    }
                    "--comp" => out.comp = value,
                    "--capture" => {
                        out.capture = Some(Mode::from_name(&value).ok_or_else(|| {
                            format!("--capture {value:?}: region, screen or window")
                        })?);
                    }
                    "--window" => {
                        out.window = Some(
                            Target::parse(&value)
                                .ok_or_else(|| format!("--window {value:?}: ID:GENERATION"))?,
                        );
                    }
                    _ => out.url = value,
                }
            }
            flag if !positional && flag.starts_with('-') => {
                return Err(format!("unknown option {flag} (see --help)"));
            }
            _ => {
                if out.path.is_some() {
                    return Err("open one image at a time".into());
                }
                out.path = Some(capture::absolute(&arg)?);
            }
        }
    }
    for (flag, name) in [("--service", &out.service), ("--comp", &out.comp)] {
        if !names::valid(name) {
            return Err(format!(
                "{flag} {name:?}: a Bus service name, 2 to 31 of a-z, 0-9 and '-', starting with a letter"
            ));
        }
    }
    if out.window.is_some() && out.capture != Some(Mode::Window) {
        return Err("--window goes with --capture window".into());
    }
    // A headless instance never takes the window's name, so Print Screen
    // never lands in an instance nobody can see.
    if !service_given {
        out.service = if out.headless {
            names::headless(&out.comp)
        } else {
            names::window(&out.comp)
        };
    }
    if out.headless && out.capture.is_some() {
        return Err("--capture needs the window; agents call screenshot.capture".into());
    }
    Ok(out)
}

/// The running instance under `service` must capture through `comp`, as
/// its `screenshot.ping` (`ping`, the reply body) says.
fn same_compositor(ping: &str, comp: &str, service: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(ping).map_err(|e| format!("{service}: unreadable ping: {e}"))?;
    match value["comp"].as_str() {
        Some(theirs) if theirs == comp => Ok(()),
        Some(theirs) => Err(format!(
            "{service} already runs for compositor {theirs:?}, not {comp:?}; the request was not handed over"
        )),
        None => Err(format!(
            "{service} does not say which compositor it captures through; the request was not handed over"
        )),
    }
}

/// Ask the instance under this launch's name which compositor it serves.
fn check_running(args: &Args) -> Result<(), String> {
    let reply = citizen::request(&args.url, &args.service, "screenshot.ping", json!({}))?;
    if reply.rc != 0 {
        return Err(format!("{}: ping refused: {}", args.service, reply.body));
    }
    same_compositor(&reply.body, &args.comp, &args.service)
}

/// Hand this launch's request to the running window.
fn forward(args: &Args) -> Result<(), String> {
    // A name shared through a hash collision must never take a request
    // meant for another compositor.
    check_running(args)?;
    let call = |verb: &str, body: serde_json::Value| {
        let reply = citizen::request(&args.url, &args.service, verb, body)?;
        if reply.rc == 0 {
            Ok(())
        } else {
            Err(format!("{verb} refused (rc {}): {}", reply.rc, reply.body))
        }
    };
    if let Some(mode) = args.capture {
        // The receiver checks the compositor too: it may have changed
        // hands since the ping.
        let mut body = json!({"mode": mode.name(), "comp": args.comp});
        if let Some(window) = args.window {
            body["window"] = json!(window);
        }
        return call("screenshot.take", body);
    }
    if let Some(path) = &args.path {
        call("screenshot.open", json!({"path": path}))?;
    }
    citizen::forward(&args.url, &args.service, SERVICE)
}

fn window(args: &Args) -> Result<(), String> {
    // The desktop profile's change hints: the session look (shell.rs).
    let topics = [::settings::summary_topic(::settings::follow::PROFILE)];
    let (handle, deliveries) =
        citizen::start_with_macros(SERVICE, &args.service, &args.url, &topics)?;
    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([1100.0, 740.0])
            .with_min_inner_size([760.0, 450.0]),
        ..eframe::NativeOptions::default()
    };
    let start = Start {
        path: args.path.clone(),
        capture: args.capture.map(|mode| (mode, args.window)),
    };
    let comp = args.comp.clone();
    let shell_bus = handle.clone();
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::install(&cc.egui_ctx, &theme);
            let shell = Shell::new(
                cc.egui_ctx.clone(),
                theme,
                shell_bus,
                deliveries,
                comp,
                start,
            )?;
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string());
    handle.quit();
    result.and(handle.wait_done())
}

fn main() {
    buildinfo::exit_on_version!(leading);
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return;
    }
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("screenshot: {error}");
            std::process::exit(2);
        }
    };
    let result = if args.headless {
        match screenshot::headless::run(&args.service, &args.url, &args.comp, args.path.clone()) {
            // The name is taken: say whose, when another compositor's.
            Err(error) if citizen::probe(&args.url, &args.service, SERVICE) => {
                check_running(&args).and(Err(error))
            }
            result => result,
        }
    } else if citizen::probe(&args.url, &args.service, SERVICE) {
        // One window per session: hand the request to it.
        forward(&args)
    } else {
        match window(&args) {
            // Another launch won the race to register: hand it ours.
            Err(_) if citizen::probe(&args.url, &args.service, SERVICE) => forward(&args),
            result => result,
        }
    };
    if let Err(error) = result {
        eprintln!("screenshot: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn a_running_instance_must_serve_the_same_compositor() {
        let ping = |comp: &str| json!({"schema": "screenshot.v1", "comp": comp}).to_string();
        assert!(
            same_compositor(
                &ping("comp-nested"),
                "comp-nested",
                "screenshot-comp-nested"
            )
            .is_ok()
        );
        let error = same_compositor(&ping("comp-a"), "comp-b", "screenshot--1234abcd").unwrap_err();
        assert!(
            error.contains("comp-a") && error.contains("comp-b"),
            "{error}"
        );
        assert!(
            same_compositor("{}", "comp", "screenshot").is_err(),
            "an older instance that does not say"
        );
        assert!(same_compositor("not json", "comp", "screenshot").is_err());
    }

    #[test]
    fn the_command_line_is_strict() {
        assert!(args(&["--comp"]).is_err());
        assert!(args(&["--service", "bad name"]).is_err());
        assert!(args(&["/tmp/a.png", "/tmp/b.png"]).is_err());
        assert!(args(&["--capture", "everything"]).is_err());
        assert!(
            args(&["--window", "3:1"]).is_err(),
            "window needs window mode"
        );
        assert!(args(&["--headless", "--capture", "region"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        let a = args(&["--capture", "window", "--window", "3:1"]).unwrap();
        assert_eq!(a.capture, Some(Mode::Window));
        assert_eq!(
            a.window,
            Some(Target {
                id: 3,
                generation: 1
            })
        );
        let a = args(&["--headless", "--comp", "comp-test", "--", "--odd.png"]).unwrap();
        assert!(a.headless);
        assert!(a.path.unwrap().ends_with("--odd.png"));
        assert_eq!(
            a.service, "screenshot-headless-comp-test",
            "never the window's name, and per compositor"
        );
        assert_eq!(
            args(&["--headless"]).unwrap().service,
            "screenshot-headless"
        );
        for bad in [
            vec!["--comp", "comp.test"],
            vec!["--comp", "Comp"],
            vec!["--service", "x"],
            vec!["--service", "a-name-that-is-far-too-long-for-abp"],
        ] {
            assert!(args(&bad).is_err(), "{bad:?}");
        }
        let a = args(&["--headless", "--service", "shot-test"]).unwrap();
        assert_eq!(a.service, "shot-test");
        assert_eq!(args(&[]).unwrap().service, "screenshot");
        // One window per compositor: a nested compd's Print reaches its own.
        let nested = args(&["--comp", "comp-nested", "--capture", "region"]).unwrap();
        assert_eq!(nested.service, "screenshot-comp-nested");
        assert_eq!(nested.comp, "comp-nested");
        let plain = args(&["--comp", "comp", "--capture", "region"]).unwrap();
        assert_eq!(plain.service, "screenshot");
        let named = args(&["--comp", "comp-nested", "--service", "mine"]).unwrap();
        assert_eq!(named.service, "mine", "an explicit name wins");
    }
}
