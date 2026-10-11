// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ced [PATH[:LINE[:COL]]…]` opens the paths in the running instance if
//! there is one (an anonymous `ced.ping`, then `ced.open`); otherwise it
//! starts the window, registered on the Bus as `ced`.

use ced::app::App;
use ced::shell::{Bus, Shell};
use documents::config::{self, Config};
use documents::controller::Controller;
use documents::dirs::{AppDirs, COMPONENT};
use editor_model::types::Intent;

const APP_ID: &str = "dev.mixos.ced";

const HELP: &str = "ced — the MixOS Editor, a client of the `edit` Bus service\n\
Usage: ced [PATH[:LINE[:COL]]…]\n\
  --headless        no window: the controller and the `ced` Bus port only\n\
  --service NAME    register as NAME instead of `ced` (tests)\n\
  --noded-url URL   the broker (default: the session's)\n\
  --print-config    print the resolved configuration and exit\n\
  --version         print version and build hash, and nothing else\n\
Bus: serves `ced.*` and `app.describe` / `app.quit`.";

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
        service: documents::verbs::SERVICE.to_owned(),
        url: ::bus::client_helpers::resolve_noded_url(),
        paths: Vec::new(),
    };
    let mut args = args.peekable();
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
                        "--service {name:?}: 2 to 31 of a-z, 0-9 and '-', starting with a \
                         letter, and not a system service"
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

/// A name noded lets a client register (`^[a-z][a-z0-9-]{1,30}$`, outside
/// the broker's `noded-` namespace and the shape of the session names it
/// issues) that is not the service ced talks to.
fn valid_service(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && (2..=31).contains(&name.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name != "edit"
        && name != "noded"
        && !name.starts_with("noded-")
        && !session_name(name)
}

/// The shape noded reserves for the session names it issues: `t` or `c`
/// and up to seven more of a-z0-9, a dash, then 22 of base32 (a-z, 2-7).
fn session_name(name: &str) -> bool {
    let Some((prefix, suffix)) = name.split_once('-') else {
        return false;
    };
    (2..=8).contains(&prefix.len())
        && prefix.starts_with(['t', 'c'])
        && prefix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && suffix.len() == 22
        && suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// Paths resolve against this process's working directory, because a
/// forwarded `ced.open` is served by an instance with another one.
fn absolute(paths: Vec<String>) -> Vec<String> {
    let cwd = std::env::current_dir().ok();
    paths
        .into_iter()
        .map(|p| match &cwd {
            Some(cwd) if !std::path::Path::new(&p).is_absolute() => {
                cwd.join(&p).to_string_lossy().into_owned()
            }
            _ => p,
        })
        .collect()
}

/// Register on the Bus, then open the window. Losing the registration to
/// an instance that started at the same moment hands it the paths instead.
fn window(args: &Args, config: Config, paths: Vec<String>) -> Result<(), String> {
    // Deliveries before the window exists wait in the channel; the first
    // frame takes them.
    let cell: std::sync::Arc<std::sync::OnceLock<egui::Context>> = Default::default();
    let waker = cell.clone();
    let wake = std::sync::Arc::new(move || {
        if let Some(ctx) = waker.get() {
            ctx.request_repaint();
        }
    });
    let (handle, deliveries) = match ced::bus::spawn(&args.service, &args.url, wake) {
        Ok(bus) => bus,
        Err(ced::bus::StartError::NameTaken) => {
            return ced::bus::forward_open(&args.url, &args.service, &paths);
        }
        Err(error) => return Err(error.to_string()),
    };
    let dirs = AppDirs::resolve(COMPONENT);
    let session_path = dirs.as_ref().map(AppDirs::session_file);
    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &ced::label("title"))
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([480.0, 320.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::install(&cc.egui_ctx, &theme);
            let _ = cell.set(cc.egui_ctx.clone());
            let mut ctl = Controller::new(config.clone(), ced::run_id(), false);
            ctl.set_paths(
                dirs.as_ref()
                    .map(|d| d.config_file().to_string_lossy().into_owned()),
                session_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            );
            if let Some(p) = &session_path {
                ctl.set_session(documents::session::load(p));
            }
            let mut app = App::new(ctl, config.clone());
            app.config_path = dirs.as_ref().map(AppDirs::config_file);
            let transport = Box::new(Bus { handle, deliveries });
            let mut shell = Shell::new(
                cc.egui_ctx.clone(),
                theme,
                app,
                transport,
                session_path.clone(),
            );
            if !paths.is_empty() {
                let fx = shell.app.ctl.open_paths(&paths, Intent::ui(0));
                shell.app.absorb(fx);
            }
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string())
}

fn main() {
    // First, before any config read or Bus connect: `--version` reports the
    // version and the build hash and does nothing else.
    buildinfo::exit_on_version!();
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return;
    }
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("ced: {e}");
            std::process::exit(2);
        }
    };
    let dirs = AppDirs::resolve(COMPONENT);
    let (config, note) = match &dirs {
        Some(d) => config::load(&d.config_file()),
        None => (
            Config::default(),
            Some("no HOME: the app directories are unresolved; using defaults".to_owned()),
        ),
    };
    if let Some(note) = &note {
        eprintln!("ced: {note}");
    }
    if args.print_config {
        if let Some(d) = &dirs {
            println!("-- {}", d.config_file().display());
        }
        println!("{}", config.to_json());
        return;
    }
    let paths = absolute(args.paths.clone());
    let result = if args.headless {
        ced::headless::run(&args.service, &args.url, config, &paths)
    } else if ced::bus::probe_running(&args.url, &args.service) {
        // One window per session: hand the paths to the running one.
        ced::bus::forward_open(&args.url, &args.service, &paths)
    } else {
        window(&args, config, paths)
    };
    if let Err(error) = result {
        eprintln!("ced: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_paths() {
        let args = parse(
            ["--headless", "--service", "ced-test", "a.md", "--", "--odd"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert!(args.headless);
        assert_eq!(args.service, "ced-test");
        assert_eq!(args.paths, ["a.md", "--odd"]);
        for bad in [
            vec!["--bogus"],
            vec!["--service", "bad name"],
            vec!["--service", "ced.test"],
            vec!["--service", "edit"],
            vec!["--service", "noded-review"],
            vec!["--service", "t1000-abcdefghijklmnopqrstuv"],
            vec!["--service", "Ced"],
            vec!["--service"],
        ] {
            assert!(parse(bad.into_iter().map(str::to_owned)).is_err());
        }
    }
}
