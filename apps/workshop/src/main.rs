// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
mod wake;

use inspector::bus;
use inspector::model::APP_ID;
use workshop::{label, shell::Shell};

#[derive(Debug, Clone)]
struct Settings {
    app: Option<String>,
    id: Option<String>,
    headless: bool,
    service: String,
    comp: String,
    url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            app: None,
            id: None,
            headless: false,
            service: "workshop".into(),
            comp: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            url: ::bus::client_helpers::resolve_noded_url(),
        }
    }
}

fn parse(args: impl Iterator<Item = String>) -> Result<Settings, String> {
    let mut settings = Settings::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        if arg == "--headless" {
            settings.headless = true;
            continue;
        }
        let value = args
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--app" => settings.app = Some(value),
            "--id" => settings.id = Some(value),
            "--noded-url" => settings.url = value,
            "--service" => settings.service = value,
            "--comp" => settings.comp = value,
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
    let (handle, deliveries) = bus::start(&settings.service, &settings.url, &topics)?;
    let theme = toolkit::Theme::load();
    if settings.headless {
        // The same engine, effect runtime and delivery ordering, with no
        // platform integration, renderer or native window. Repaints wake the
        // logic thread; no timer polls the broker or the engine.
        let ctx = egui::Context::default();
        let wake = wake::Wake::install(&ctx);
        let mut shell = Shell::new(
            ctx.clone(),
            theme,
            handle.clone(),
            deliveries,
            settings.comp.clone(),
        )?;
        if let Some(app) = &settings.app {
            shell.engine_mut().queue_workshop(
                "macros.open",
                serde_json::json!({"app":app,"id":settings.id}),
                None,
            );
        }
        loop {
            wake.begin();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| shell.logic(ui.ctx()));
            for viewport in output.viewport_output.values() {
                wake.schedule(viewport.repaint_delay);
            }
            // No renderer consumes the atlas updates in headless mode.
            output.textures_delta.clear();
            if shell.exiting() {
                break;
            }
            wake.wait();
        }
        drop(shell);
        return handle.wait_done();
    }
    let saved = toolkit::saved::Store::<workshop::saved::Arrangement>::load(
        config::AppDirs::resolve("workshop").map(|d| d.state()),
        1,
    );
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: saved.window.viewport(
            toolkit::titlebar::viewport(APP_ID, &label("title"))
                .with_inner_size([1100.0, 760.0])
                .with_min_inner_size([760.0, 500.0]),
        ),
        ..eframe::NativeOptions::default()
    };
    let comp = settings.comp.clone();
    let shell_bus = handle.clone();
    let initial = settings
        .app
        .as_ref()
        .map(|app| serde_json::json!({"app":app,"id":settings.id}));
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            toolkit::install(&cc.egui_ctx, &theme);
            let mut shell = Shell::new(cc.egui_ctx.clone(), theme, shell_bus, deliveries, comp)?
                .with_saved(saved);
            if let Some(args) = initial {
                shell.engine_mut().queue_workshop("macros.open", args, None);
            }
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
            "workshop — native ABP service and command browser\nUsage: workshop [--headless] [--noded-url URL] [--service NAME] [--comp NAME]"
        );
        return;
    }
    let result = parse(std::env::args().skip(1)).and_then(|settings| {
        // One window per session: a second launch raises the first.
        if !settings.headless && bus::probe(&settings.url, &settings.service) {
            if let Some(app) = &settings.app {
                bus::open(
                    &settings.url,
                    &settings.service,
                    serde_json::json!({"app":app,"id":settings.id}),
                )?;
            }
            match bus::forward(&settings.url, &settings.service) {
                Ok(()) => return Ok(()),
                Err(error) if bus::probe(&settings.url, &settings.service) => return Err(error),
                Err(_) => {} // The previous instance exited between probe and activation.
            }
        }
        match run(&settings) {
            Err(_) if !settings.headless && bus::probe(&settings.url, &settings.service) => {
                bus::forward(&settings.url, &settings.service)
            }
            result => result,
        }
    });
    if let Err(error) = result {
        eprintln!("workshop: {error}");
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
        ] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let settings = parse(
            ["--service", "workshop.test"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(settings.service, "workshop.test");
    }
}
