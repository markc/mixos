// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `dopus [LEFT [RIGHT]]` opens the paths in the running instance if there
//! is one (an anonymous `dopus.ping`, then `dopus.open`); otherwise it
//! starts the window, registered on the Bus as `dopus`. Without a broker
//! the window runs on its own.

use std::sync::{Arc, OnceLock};

use dopus::app::App;
use dopus::dirs::{AppDirs, COMPONENT};
use dopus::shell::{Bus, Shell};
use dopus::{APP_ID, bus, label, verbs};
use dopus_core::DopusCore;

const HELP: &str = "dopus — the MixOS twin-pane file manager\n\
Usage: dopus [LEFT [RIGHT]]\n\
  --headless        no window: the core and the `dopus` Bus port only\n\
  --service NAME    register as NAME instead of `dopus` (tests)\n\
  --noded-url URL   the broker (default: the session's)\n\
  --print-config    print the resolved configuration and exit\n\
  --version         print version and build hash, and nothing else\n\
Bus: serves `dopus.*` (schema dopus.v1) and the window's drive verbs.";

struct Args {
    headless: bool,
    print_config: bool,
    service: String,
    url: String,
    paths: Vec<String>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        headless: false,
        print_config: false,
        service: verbs::SERVICE.to_owned(),
        url: ::bus::client_helpers::resolve_noded_url(),
        paths: Vec::new(),
    };
    let mut args = args;
    let mut only_paths = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            _ if only_paths => out.paths.push(a),
            "--" => only_paths = true,
            "--headless" => out.headless = true,
            "--print-config" => out.print_config = true,
            "--noded-url" => out.url = args.next().ok_or("--noded-url needs a URL")?,
            "--service" => {
                let name = args.next().ok_or("--service needs a name")?;
                if !valid_service(&name) {
                    return Err(format!(
                        "--service {name:?}: 2 to 31 of a-z, 0-9 and '-', starting with a letter"
                    ));
                }
                out.service = name;
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag} (see --help)"));
            }
            _ => out.paths.push(a),
        }
    }
    Ok(out)
}

/// A name noded lets a client register, outside its own namespace.
fn valid_service(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && (2..=31).contains(&name.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name != "noded"
        && !name.starts_with("noded-")
}

/// Paths resolve against this process's working directory, because a
/// forwarded `dopus.open` is served by an instance with another one.
fn absolute(paths: Vec<String>) -> Vec<String> {
    let cwd = std::env::current_dir().ok();
    paths
        .into_iter()
        .map(|p| {
            let expanded = dopus::dirs::expand_tilde(&p);
            match &cwd {
                Some(cwd) if expanded.is_relative() => cwd.join(expanded).display().to_string(),
                _ => expanded.display().to_string(),
            }
        })
        .collect()
}

fn window(args: &Args, dirs: Option<AppDirs>, paths: Vec<String>) -> Result<(), String> {
    // Deliveries before the window exists wait in the channel; the first
    // frame takes them.
    let cell: Arc<OnceLock<egui::Context>> = Arc::default();
    let waker = cell.clone();
    let wake: bus::Wake = Arc::new(move || {
        if let Some(ctx) = waker.get() {
            ctx.request_repaint();
        }
    });
    let bus = match bus::spawn(&args.service, &args.url, wake) {
        Ok((handle, deliveries)) => Some(Bus { handle, deliveries }),
        Err(bus::StartError::NameTaken) => {
            return bus::forward_open(&args.url, &args.service, &paths);
        }
        Err(bus::StartError::Unreachable(why)) => {
            eprintln!("dopus: running without the Bus ({why})");
            None
        }
        Err(error) => return Err(error.to_string()),
    };
    let (config, file) = dopus::config::load(dirs.as_ref().map(AppDirs::config_dir).as_deref());
    let config_path = file.as_ref().map(|f| f.path.display().to_string());
    let (mut core, core_events) = DopusCore::new(config, file);
    verbs::apply_open_paths(&mut core, &paths);
    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([420.0, 240.0]),
        ..eframe::NativeOptions::default()
    };
    let service = args.service.clone();
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            toolkit::install(&cc.egui_ctx, &theme);
            let _ = cell.set(cc.egui_ctx.clone());
            let app = App::new(core);
            let shell = Shell::new(
                cc.egui_ctx.clone(),
                theme,
                app,
                core_events,
                bus,
                service,
                config_path,
            );
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string())
}

fn main() {
    // First, before any config read or Bus connect.
    buildinfo::exit_on_version!();
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return;
    }
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("dopus: {e}");
            std::process::exit(2);
        }
    };
    let dirs = AppDirs::resolve(COMPONENT);
    if args.print_config {
        let (config, file) = dopus::config::load(dirs.as_ref().map(AppDirs::config_dir).as_deref());
        if let Some(file) = &file {
            println!("-- {}", file.path.display());
        }
        println!("{}", dopus::config::to_json(&config));
        return;
    }
    let paths = absolute(args.paths.clone());
    let result = if args.headless {
        let (config, file) = dopus::config::load(dirs.as_ref().map(AppDirs::config_dir).as_deref());
        dopus::headless::run(config, file, dirs, &args.service, &args.url, &paths)
    } else if bus::probe_running(&args.url, &args.service) {
        // One window per session: hand the paths to the running one.
        bus::forward_open(&args.url, &args.service, &paths)
    } else {
        window(&args, dirs, paths)
    };
    if let Err(error) = result {
        eprintln!("dopus: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_paths() {
        let args = parse(
            ["--headless", "--service", "dopus-test", "a", "--", "--odd"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert!(args.headless);
        assert_eq!(args.service, "dopus-test");
        assert_eq!(args.paths, ["a", "--odd"]);
        for bad in [
            vec!["--bogus"],
            vec!["--service", "bad name"],
            vec!["--service", "noded-x"],
            vec!["--service", "Dopus"],
            vec!["--service"],
        ] {
            assert!(parse(bad.into_iter().map(str::to_owned)).is_err());
        }
    }
}
