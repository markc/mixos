// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email: the reserved MixOS mail client — a pure JMAP client (mail,
//! calendar, contacts) over HTTPS to any RFC 8620/8621-shaped server.
//!
//! The behaviour is the `jmap` engine crate (the wire protocol); this
//! crate is the egui shell around it (AGENTS.md §4.1): [`view`] draws the
//! state and returns what the person did, [`state`] applies both those
//! [`view::UiEvent`]s and the shell's [`shell::Event`] completions and
//! schedules [`work::Work`], and [`shell`] performs that work on a small
//! tokio runtime — one repaint per completion, nothing polls.
//!
//! The accounts are settingsd's (`email.*`, configured in Prefs); the app
//! follows them and [`writer`] changes them with fenced applies. Their
//! passwords live only in [`secrets`], the app's private file. [`config`]
//! persists the window's shape and the last accounts seen under the
//! poison-pill law, and migrates the single account an older config held.
//!
//! The one architectural exception this app owns: it speaks HTTPS itself,
//! from its own effects runtime, where every other app reaches services
//! through the Bus. A pure JMAP client must — the server is whatever
//! account the person points it at, not a service this node hosts.

pub mod actions;
pub mod attachments;
pub mod commands;
pub mod composer;
pub mod config;
pub mod documents;
pub mod drafts;
pub mod previews;
pub mod print;
pub mod quote;
pub mod reader;
pub mod secrets;
pub mod shell;
pub mod state;
pub mod suggest;
pub mod view;
pub mod work;
pub mod writer;

use toolkit::Strings;

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/email.ftl");

/// The loaded catalogue.
pub fn strings() -> Strings {
    Strings::new(CATALOGUE)
}

thread_local! {
    static STRINGS: Strings = strings();
}

/// `key` from the catalogue.
pub fn label(key: &str) -> String {
    STRINGS.with(|s| s.get(key))
}

/// `key` from the catalogue with named arguments.
pub fn label_with(key: &str, args: &[(&str, &str)]) -> String {
    STRINGS.with(|s| s.with(key, args))
}

/// The app name: the Bus service and the drive-surface prefix.
pub const APP: &str = "email";

/// The Wayland app id (the one place the brand appears).
pub const APP_ID: &str = "dev.mixos.email";

/// What a launch was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub compose: bool,
    pub document: Option<std::path::PathBuf>,
    /// The Bus service name to register as.
    pub service: String,
    /// The noded URL to reach the broker through.
    pub url: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            compose: false,
            document: None,
            service: APP.into(),
            url: ::bus::client_helpers::resolve_noded_url(),
        }
    }
}

/// Parse the command line, house style: manual, with a HELP string.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut settings = Args::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--compose" {
            settings.compose = true;
            if args.peek().is_some_and(|v| !v.starts_with('-')) {
                let path = std::path::PathBuf::from(args.next().unwrap());
                settings.document = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map_err(|e| e.to_string())?
                        .join(path)
                });
            }
            continue;
        }
        let value = args
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--noded-url" => settings.url = value,
            "--service" => settings.service = value,
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    let Args { service, .. } = &settings;
    if service.is_empty()
        || !service
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err("invalid service name".into());
    }
    Ok(settings)
}

/// Run the window. One instance per session: a second launch raises the
/// first (the caller has already probed and forwarded if it found one).
pub fn run(args: &Args) -> Result<(), String> {
    // The desktop profile's change hints: the session look (shell.rs).
    let topics = [::settings::summary_topic(::settings::follow::PROFILE)];
    let (handle, deliveries) = citizen::start_with_macros(APP, &args.service, &args.url, &topics)?;
    let theme = toolkit::Theme::load();
    let mut viewport = if args.compose {
        toolkit::titlebar::viewport("dev.mixos.email.compose-1", &label("compose-title"))
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([640.0, 480.0])
    } else {
        toolkit::titlebar::viewport(APP_ID, &label("title"))
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([820.0, 520.0])
    };
    if args.compose {
        let store = toolkit::saved::Store::<serde_json::Value>::load(
            ::config::AppDirs::resolve(APP).map(|d| d.state()),
            0,
        );
        if let Some(frame) = store.windows.get("compose-1") {
            viewport = frame.viewport(viewport);
        }
    }
    let compose = args.compose;
    let document = args.document.clone();
    let options = eframe::NativeOptions {
        // Client-side decorations: the toolkit title bar is the window frame.
        viewport,
        ..eframe::NativeOptions::default()
    };
    let shell_bus = handle.clone();
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            toolkit::install(&cc.egui_ctx, &theme);
            let mut shell = shell::Shell::new(cc.egui_ctx.clone(), theme, shell_bus, deliveries)?;
            shell.compose_launch(compose, document);
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| e.to_string());
    handle.quit();
    result.and(handle.wait_done())
}

/// Forward both initial probes and registration races through the same path.
pub fn forward(args: &Args) -> Result<(), String> {
    if !args.compose {
        return citizen::forward(&args.url, &args.service, APP);
    }
    let reply = citizen::request(
        &args.url,
        &args.service,
        "email.compose",
        compose_request(args),
    )?;
    if reply.rc == 0 {
        Ok(())
    } else {
        Err(reply.body)
    }
}

fn compose_request(args: &Args) -> serde_json::Value {
    args.document.as_ref().map_or_else(
        || serde_json::json!({}),
        |path| serde_json::json!({"document":path}),
    )
}

#[cfg(test)]
mod compose_args_tests {
    use super::*;

    #[test]
    fn compose_accepts_a_document_and_following_options() {
        let args = parse(
            ["--compose", "message.md", "--service", "email-test"]
                .map(str::to_owned)
                .into_iter(),
        )
        .unwrap();
        assert!(args.compose);
        assert_eq!(args.service, "email-test");
        assert_eq!(
            args.document,
            Some(std::env::current_dir().unwrap().join("message.md"))
        );
        let args = parse(
            ["--compose", "--service", "email-test"]
                .map(str::to_owned)
                .into_iter(),
        )
        .unwrap();
        assert!(args.compose && args.document.is_none());
        assert!(!parse(std::iter::empty()).unwrap().compose);
        assert!(parse(["--compose", "--unknown"].map(str::to_owned).into_iter()).is_err());
    }
}
