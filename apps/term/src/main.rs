// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `term`: the Terminal window, registered on the Bus as `term`. With
//! `--headless`, the same tab set and `term.*` commands with no window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use term::app::App;
use term::bus::{self, WindowBus};
use term::clipboard::Clipboard;
use term::shell::Shell;
use term::{APP_ID, label};
use term_core::{
    config,
    font::FontSize,
    native_lane::NativeLane,
    raster::Raster,
    session_fd,
    tabs::{self, TabSet},
    terminal::Wake,
};
use wgpu::rwh::HasDisplayHandle;

/// The Bus name and command namespace this frontend owns.
const SERVICE: &str = "term";

const HELP: &str = "term — Terminal, the MixOS terminal: tabs, split panes, a GPU-drawn grid\n\
Usage: term [--headless] [--print-config]\n\
Settings: the desktop profile's terminal.* (settingsd; Prefs), defaults without it\n\
Font: TERM_SPIKE_FONT=/path/to/font.ttf, TERM_FONT_PX=<6..48> until settingsd answers\n\
Keys: Ctrl+Shift+T/W new/close tab, Ctrl+PageUp/PageDown change tab,\n\
\x20     Ctrl+Shift+E/O split side by side/stacked, Ctrl+Shift+X close pane,\n\
\x20     Ctrl+Shift+arrows move focus, Ctrl+Shift+Q quit,\n\
\x20     Tab / Shift+Tab switch split panes; Ctrl+Tab / Ctrl+Shift+Tab also cycle,\n\
\x20     Ctrl+Shift+C copy; Ctrl+Shift+V or Shift+Insert paste the clipboard\n\
\x20     Drag selects; double/triple click word/line; middle click pastes primary\n\
\x20     Shift+mouse overrides application mouse reporting\n\
\x20     Ctrl+plus/equal/minus/0 (and Ctrl+wheel) font size\n\
\x20     Wheel scrolls; Shift+PageUp/PageDown page history\n\
\x20     Shift+Home/End history top/bottom (primary screen only)\n\
\x20     Input returns to bottom; single-pane Tab or Ctrl+I goes to the shell\n\
TERM_NOTIFY=0: no desktop notification when a pane's shell exits\n\
--headless: the tab set and the term.* commands, no window\n\
--version: print version and build hash, and nothing else\n\
--print-config: print resolved startup settings and exit\n\
Bus: serves `term` / `term.*`, the term.ui.* drive commands, term.actions and term.execute";

fn main() {
    let mut cli_args: Vec<String> = std::env::args().collect();
    let _logging = toolkit::debug::init("term", "warn", &mut cli_args).unwrap_or_else(|error| {
        toolkit::debug::failure("term", &error);
        std::process::exit(2);
    });
    // First, before the inherited-fd quarantine, the TERM probe and the
    // window: `--version` reports the version and the build hash and does
    // nothing else, whether or not a term is running or a display exists.
    buildinfo::exit_on_version!(leading);
    session_fd::quarantine_inherited();
    let args: Vec<String> = cli_args.into_iter().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}\n{}", toolkit::debug::HELP);
        return;
    }
    if let Some(unknown) = args
        .iter()
        .find(|arg| !matches!(arg.as_str(), "--headless" | "--print-config"))
    {
        eprintln!("term: unknown option {unknown} (see --help)");
        std::process::exit(2);
    }
    // The defaults until settingsd's `terminal.*` settings arrive (the
    // window follows them; --headless keeps these).
    let settings = resolve_config(
        config::Config::default(),
        std::env::var("TERM_FONT_PX").ok().as_deref(),
        config::selected_term(),
    );
    if args.iter().any(|arg| arg == "--print-config") {
        println!(
            "{}",
            serde_json::to_string_pretty(&settings).expect("validated config")
        );
        return;
    }
    let result = if args.iter().any(|arg| arg == "--headless") {
        headless(settings)
    } else if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        Err("term requires a native Wayland session (or --headless)".into())
    } else {
        run(settings)
    };
    if let Err(error) = result {
        toolkit::debug::failure("term", &error);
        std::process::exit(1);
    }
}

fn resolve_config(
    mut config: config::Config,
    env_font: Option<&str>,
    term: &'static str,
) -> config::Settings {
    if let Some(px) = env_font
        .and_then(|value| value.parse().ok())
        .filter(|px| config::valid_font(*px))
    {
        config.font_px = px;
    }
    config::Settings {
        config,
        term: term.to_owned(),
    }
}

/// Whether a pane's shell exiting sends a desktop notification.
fn notify_enabled() -> bool {
    std::env::var("TERM_NOTIFY")
        .map(|value| value != "0")
        .unwrap_or(true)
}

fn run(settings: config::Settings) -> Result<(), String> {
    // Scale 1 to start; the window rebuilds the raster at its real scale
    // before the first frame draws, so glyphs are rasterised at physical
    // resolution. Building it here means a missing font fails before a
    // window appears.
    let raster = Raster::new(1.0, settings.config.font_px, settings.config.cursor)?;
    let font = FontSize::new(settings.config.font_px);
    let tabs = Arc::new(Mutex::new(TabSet::starting(settings.clone())));
    let (cleanup, reaper) = tabs::Cleanup::start().map_err(|e| format!("cleanup worker: {e}"))?;

    // Every PTY, resize, pane exit and Bus mutation wakes the window: one
    // flag, set on any thread, and a repaint request on its rising edge, so
    // a flood of PTY output asks for one frame, not one per write.
    let woken = Arc::new(AtomicBool::new(true));
    let window: Arc<OnceLock<egui::Context>> = Arc::default();
    let core_wake: Wake = {
        let (woken, window) = (woken.clone(), window.clone());
        Arc::new(move || {
            if !woken.swap(true, Ordering::AcqRel)
                && let Some(ctx) = window.get()
            {
                ctx.request_repaint();
            }
        })
    };
    let wake: Arc<dyn Fn() + Send + Sync> = {
        let window = window.clone();
        Arc::new(move || {
            if let Some(ctx) = window.get() {
                ctx.request_repaint();
            }
        })
    };
    tabs.lock().expect("tabs").set_wake(core_wake);
    let native = NativeLane::start_background(tabs.clone(), settings, cleanup.clone())
        .map_err(|e| format!("native startup worker: {e}"))?;

    // A pane whose shell exits on its own is reaped on the UI thread and
    // handed to the Bus thread, which emits interact.notify. TERM_NOTIFY=0
    // drops the sender, so notes are never queued.
    let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
    // Bounded: a window that falls behind (off screen, it draws no frames)
    // has the lane answer BUSY, and each request expires after
    // WINDOW_REPLY_TIMEOUT.
    let (events_tx, events_rx) = std::sync::mpsc::sync_channel(64);
    let outbox = Arc::new(citizen::outbox::Outbox::default());
    let link = Arc::new(OnceLock::new());
    // The `term.*` surface, and the window's commands on the same name. With
    // no broker the thread says so once and returns; the terminal works.
    let bus_thread = term_core::bus::start_with_window(
        SERVICE,
        tabs.clone(),
        cleanup.clone(),
        notify_rx,
        term_core::bus::Window {
            answers: bus::answers,
            events: events_tx,
            wake: wake.clone(),
            outbox: outbox.clone(),
            link: link.clone(),
            reply_timeout: term_core::bus::WINDOW_REPLY_TIMEOUT,
        },
    );

    let theme = toolkit::Theme::load();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport: toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([900.0, 560.0]),
        ..eframe::NativeOptions::default()
    };
    let notify = notify_enabled().then_some(notify_tx);
    let (app_tabs, app_cleanup) = (tabs.clone(), cleanup.clone());
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            let render_state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or("term draws its grid with wgpu, and eframe has no wgpu renderer")?;
            term_render::install(render_state);
            toolkit::install(&cc.egui_ctx, &theme);
            let _ = window.set(cc.egui_ctx.clone());
            let mut app = App::new(app_tabs, app_cleanup, notify, woken, raster, font);
            app.rescale(cc.egui_ctx.pixels_per_point());
            let display = cc.display_handle().ok().map(|handle| handle.as_raw());
            // SAFETY: eframe keeps its display open until after the app,
            // and with it this clipboard, is dropped.
            let clipboard = unsafe { Clipboard::new(display, wake.clone()) };
            let bus = WindowBus::new(link, events_rx, outbox, wake);
            Ok(Box::new(Shell::new(
                cc.egui_ctx.clone(),
                theme,
                app,
                bus,
                clipboard,
            )))
        }),
    );

    // Shut the tabs (which releases the Bus loop), finish bounded Bus
    // replies, then reap each pane while the native actor is still serving
    // its revoke acknowledgements.
    let removed = tabs.lock().expect("tabs").shutdown();
    cleanup.submit(removed);
    let _ = bus_thread.join();
    let native = native
        .join()
        .map_err(|_| "native startup worker panicked")?;
    native.release_cleanup();
    drop(cleanup);
    let _ = reaper.join();
    let startup = native.startup_result();
    drop(native);
    startup?;
    result.map_err(|error| error.to_string())
}

/// The tab set and the `term.*` commands with no window: panes keep the size
/// they opened at, and exited shells are reaped as they go. Ends when the
/// last tab closes (`term.tab.close`).
fn headless(settings: config::Settings) -> Result<(), String> {
    let tabs = Arc::new(Mutex::new(TabSet::starting(settings.clone())));
    let (cleanup, reaper) = tabs::Cleanup::start().map_err(|e| format!("cleanup worker: {e}"))?;
    let woken = Arc::new(AtomicBool::new(true));
    let thread = std::thread::current();
    let wake: Wake = {
        let woken = woken.clone();
        Arc::new(move || {
            if !woken.swap(true, Ordering::AcqRel) {
                thread.unpark();
            }
        })
    };
    tabs.lock().expect("tabs").set_wake(wake);
    let native = NativeLane::start_background(tabs.clone(), settings, cleanup.clone())
        .map_err(|e| format!("native startup worker: {e}"))?;
    let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
    let notify = notify_enabled().then_some(notify_tx);
    let bus_thread = term_core::bus::start(SERVICE, tabs.clone(), cleanup.clone(), notify_rx);
    loop {
        if !woken.swap(false, Ordering::AcqRel) {
            std::thread::park();
            continue;
        }
        let (removed, notes) = tabs.lock().expect("tabs").reap_exited();
        cleanup.submit(removed);
        if let Some(notify) = &notify {
            for note in notes {
                let _ = notify.send(note);
            }
        }
        let tabs = tabs.lock().expect("tabs");
        if tabs.is_empty() && !tabs.is_starting() {
            break;
        }
    }
    drop(notify);
    let removed = tabs.lock().expect("tabs").shutdown();
    cleanup.submit(removed);
    let _ = bus_thread.join();
    let native = native
        .join()
        .map_err(|_| "native startup worker panicked")?;
    native.release_cleanup();
    drop(cleanup);
    let _ = reaper.join();
    let startup = native.startup_result();
    drop(native);
    startup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_env_font_size_overrides_the_config_only_when_it_is_valid() {
        let base = config::Config {
            font_px: 13.0,
            ..config::Config::default()
        };
        assert_eq!(
            resolve_config(base, Some("18.5"), "xterm").config.font_px,
            18.5
        );
        for rejected in ["5.9", "48.1", "", "eighteen", "nan", "inf"] {
            assert_eq!(
                resolve_config(base, Some(rejected), "xterm").config.font_px,
                13.0,
                "accepted {rejected}"
            );
        }
        assert_eq!(resolve_config(base, None, "xterm").config.font_px, 13.0);
    }
}
