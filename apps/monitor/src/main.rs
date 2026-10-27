// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use monitor::{label, shell::Shell};
use vitals::model::APP_ID;

/// The app name: the worker thread, and the `monitor.ping` /
/// `monitor.show` activation commands.
const APP: &str = "monitor";

#[derive(Debug, Clone)]
struct Settings {
    service: String,
    comp: String,
    url: String,
    node: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            service: "monitor".into(),
            comp: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            url: ::bus::client_helpers::resolve_noded_url(),
            node: None,
        }
    }
}

fn parse(args: impl Iterator<Item = String>) -> Result<Settings, String> {
    let mut settings = Settings::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--noded-url" => settings.url = value,
            "--service" => settings.service = value,
            "--comp" => settings.comp = value,
            "--node" if vitals::model::valid_node(&value) => settings.node = Some(value),
            "--node" => return Err("invalid node name".into()),
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    for name in [&settings.service, &settings.comp] {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Err("invalid service name".into());
        }
    }
    Ok(settings)
}

fn run(settings: &Settings) -> Result<(), String> {
    // The desktop profile's change hints: the session look (shell.rs).
    let topics = [::settings::summary_topic(::settings::follow::PROFILE)];
    let (handle, deliveries) =
        citizen::start_with_macros(APP, &settings.service, &settings.url, &topics)?;
    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([720.0, 480.0]),
        ..eframe::NativeOptions::default()
    };
    let comp = settings.comp.clone();
    let node = settings.node.clone();
    let shell_bus = handle.clone();
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            toolkit::install(&cc.egui_ctx, &theme);
            let mut shell = Shell::new(cc.egui_ctx.clone(), theme, shell_bus, deliveries, comp)?;
            if node.is_some() {
                shell.watch(node);
            }
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string());
    handle.quit();
    result.and(handle.wait_done())
}

fn main() {
    let mut cli_args: Vec<String> = std::env::args().collect();
    let _logging = toolkit::debug::init("monitor", "warn", &mut cli_args).unwrap_or_else(|error| {
        toolkit::debug::failure("monitor", &error);
        std::process::exit(2);
    });
    buildinfo::exit_on_version!(leading);
    if matches!(cli_args.get(1).map(String::as_str), Some("--help" | "-h")) {
        println!(
            "monitor — processes and performance from monitord, on this node or any mesh node\nUsage: monitor [--node NAME] [--noded-url URL] [--service NAME] [--comp NAME]"
        );
        println!("{}", toolkit::debug::HELP);
        return;
    }
    let result = parse(cli_args.into_iter().skip(1)).and_then(|settings| {
        // One window per session: a second launch raises the first.
        if citizen::probe(&settings.url, &settings.service, APP) {
            match citizen::forward(&settings.url, &settings.service, APP) {
                Ok(()) => return Ok(()),
                Err(error) if citizen::probe(&settings.url, &settings.service, APP) => {
                    return Err(error);
                }
                Err(_) => {} // The previous instance exited between probe and activation.
            }
        }
        match run(&settings) {
            Err(_) if citizen::probe(&settings.url, &settings.service, APP) => {
                citizen::forward(&settings.url, &settings.service, APP)
            }
            result => result,
        }
    });
    if let Err(error) = result {
        toolkit::debug::failure("monitor", &error);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_missing_and_unsafe_options() {
        for args in [
            vec!["--noded-url"],
            vec!["--bogus", "x"],
            vec!["--service", "bad name"],
            vec!["--node", "a.b"],
        ] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let settings = parse(["--node", "alpha"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(settings.node.as_deref(), Some("alpha"));
    }
}
