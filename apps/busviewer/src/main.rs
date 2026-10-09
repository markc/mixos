// SPDX-License-Identifier: MIT OR Apache-2.0
use busviewer::{label, shell::Shell};
use inspector::bus;
use inspector::model::APP_ID;

#[derive(Debug, Clone)]
struct Settings {
    service: String,
    comp: String,
    url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            service: "busviewer".into(),
            comp: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            url: ::bus::client_helpers::resolve_noded_url(),
        }
    }
}

fn parse(args: impl Iterator<Item = String>) -> Result<Settings, String> {
    let mut settings = Settings::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        let value = args.next().filter(|v| !v.is_empty()).ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--noded-url" => settings.url = value,
            "--service" => settings.service = value,
            "--comp" => settings.comp = value,
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    for name in [&settings.service, &settings.comp] {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
            return Err("invalid service name".into());
        }
    }
    Ok(settings)
}

fn run(settings: &Settings) -> Result<(), String> {
    let (handle, deliveries) = bus::start(&settings.service, &settings.url)?;
    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([760.0, 500.0]),
        ..eframe::NativeOptions::default()
    };
    let comp = settings.comp.clone();
    let shell_bus = handle.clone();
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::install(&cc.egui_ctx, &theme);
            let shell = Shell::new(cc.egui_ctx.clone(), theme, shell_bus, deliveries, comp)?;
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string());
    handle.quit();
    result.and(handle.wait_done())
}

fn main() {
    buildinfo::exit_on_version!(leading);
    if matches!(std::env::args().nth(1).as_deref(), Some("--help" | "-h")) {
        println!(
            "busviewer — native ABP service and verb browser\nUsage: busviewer [--noded-url URL] [--service NAME] [--comp NAME]"
        );
        return;
    }
    let result = parse(std::env::args().skip(1)).and_then(|settings| {
        // One window per session: a second launch raises the first.
        if bus::probe(&settings.url, &settings.service) {
            match bus::forward(&settings.url, &settings.service) {
                Ok(()) => return Ok(()),
                Err(error) if bus::probe(&settings.url, &settings.service) => return Err(error),
                Err(_) => {} // The previous instance exited between probe and activation.
            }
        }
        match run(&settings) {
            Err(_) if bus::probe(&settings.url, &settings.service) => bus::forward(&settings.url, &settings.service),
            result => result,
        }
    });
    if let Err(error) = result {
        eprintln!("busviewer: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_missing_and_unsafe_options() {
        for args in [vec!["--noded-url"], vec!["--bogus", "x"], vec!["--service", "bad name"]] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let settings = parse(["--service", "busviewer.test"].into_iter().map(str::to_owned)).unwrap();
        assert_eq!(settings.service, "busviewer.test");
    }
}
