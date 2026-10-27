// SPDX-License-Identifier: MIT OR Apache-2.0
//! Process-local debug control. No properties, persistence or stats recorder.
use crate::{LogDefaults, LogError, LogHandle, LogOpts, LogReloadHandle, StatsOpts, TriState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::EnvFilter;

pub const HELP: &str = "--debug[=LEVEL]: off|error|warn|info|debug|trace; --debug means debug. Default: the app baseline (usually warn). RUST_LOG overrides the baseline.";

#[inline]
pub fn received(service: &str, command: &str) {
    tracing::debug!(service, command, "Bus command received");
}

#[inline]
pub fn answered(service: &str, command: &str, rc: u8) {
    tracing::debug!(service, command, rc, "Bus command answered");
}

#[inline]
pub fn failure(app: &str, error: &impl std::fmt::Display) {
    tracing::error!(app, %error, "application error");
}

/// Own targets use Rust crate names (hyphens become underscores).
const WORKSPACE_TARGETS: &[&str] = &[
    "action",
    "annotation",
    "assets",
    "backdrop",
    "blobd",
    "buildinfo",
    "bus",
    "ced",
    "citizen",
    "compd",
    "compd_wlcs",
    "comp_model",
    "comp_service",
    "config",
    "daemon",
    "davproto",
    "decor",
    "design",
    "dispatcher",
    "dns_core",
    "dnsd",
    "documents",
    "dopus",
    "dopus_core",
    "drivers",
    "edges",
    "edit",
    "editd",
    "editor",
    "editor_model",
    "effects",
    "email",
    "expr",
    "files",
    "frames",
    "graphics",
    "headless",
    "hostd",
    "indexd",
    "inputd",
    "inputd_core",
    "inputd_schema",
    "inspector",
    "jmap",
    "kms",
    "ledger",
    "logging",
    "macros",
    "maild",
    "maild_auth",
    "maild_bayesian",
    "maild_rules",
    "mds",
    "mesh",
    "mesh_sign",
    "mesh_trust",
    "mimedb",
    "mix",
    "mix_shell",
    "mixos_mcp",
    "mixtrayd",
    "model",
    "monitor",
    "monitord",
    "motion",
    "native",
    "nested",
    "noded",
    "outputs",
    "paths",
    "policy",
    "policy_host",
    "portald",
    "preferences",
    "prefs",
    "props",
    "protocols",
    "recorder",
    "render_gles",
    "scene",
    "scene_host",
    "scenegraph",
    "screencopy",
    "screenshot",
    "seat",
    "settings",
    "settingsd",
    "slots",
    "strict",
    "surfaces",
    "syntax",
    "system",
    "term",
    "term_core",
    "term_render",
    "test_broker",
    "testkit",
    "thumbs",
    "toolkit",
    "ui",
    "vitals",
    "wallpaper",
    "webd",
    "wlcs",
    "world",
    "workshop",
    "x11_wm",
];
// EnvFilter matches target prefixes. These resolved dependencies share an
// own-crate prefix, so give their longer names explicit dependency levels.
const PREFIX_DEPENDENCIES: &[&str] = &["strict_num", "system_deps", "termcolor"];
const CLAMPED_TARGETS: &[&str] = &[
    "wgpu_core",
    "wgpu_hal",
    "naga",
    "winit",
    "calloop",
    "egui",
    "eframe",
    "hyper",
    "tokio",
    "rustls",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    fn parse(value: &str) -> Result<Self, LogError> {
        serde_json::from_value(json!(value)).map_err(|_| {
            LogError::InvalidFilter(format!(
                "unknown debug level {value:?}; expected off|error|warn|info|debug|trace"
            ))
        })
    }
}

/// Replace the entire filter; dependencies never inherit own-crate debug/trace.
pub fn filter_for(level: Level) -> String {
    match level {
        Level::Off | Level::Error | Level::Warn => level.name().to_owned(),
        Level::Info | Level::Debug | Level::Trace => {
            let mut filter = String::from("info");
            for target in WORKSPACE_TARGETS {
                filter.push_str(&format!(",{target}={}", level.name()));
            }
            for target in PREFIX_DEPENDENCIES {
                filter.push_str(&format!(",{target}=info"));
            }
            for target in CLAMPED_TARGETS {
                filter.push_str(&format!(",{target}=warn"));
            }
            filter
        }
    }
}

struct Control {
    reload: LogReloadHandle,
    baseline: String,
    filter: String,
}

impl Control {
    fn state(&self) -> Value {
        let level = parse_filter(&self.filter)
            .expect("validated filter")
            .max_level_hint()
            .unwrap_or(tracing_subscriber::filter::LevelFilter::TRACE);
        json!({"level":level.to_string().to_lowercase(),
            "on":level >= tracing_subscriber::filter::LevelFilter::DEBUG,
            "filter":self.filter,"baseline":self.baseline})
    }

    fn set(&mut self, request: SetRequest) -> Result<Value, LogError> {
        let filter = match (request.level, request.on) {
            (Some(level), None) => filter_for(level),
            (None, Some(true)) => filter_for(Level::Debug),
            (None, Some(false)) => self.baseline.clone(),
            _ => {
                return Err(LogError::InvalidFilter(
                    "provide exactly one of level or on".into(),
                ));
            }
        };
        self.reload.reload_filter(parse_filter(&filter)?)?;
        self.filter = filter;
        tracing::debug!(filter = %self.filter, "debug mode changed");
        Ok(self.state())
    }
}

static CONTROL: OnceLock<Mutex<Control>> = OnceLock::new();

fn parse_filter(filter: &str) -> Result<EnvFilter, LogError> {
    EnvFilter::try_new(filter).map_err(|e| LogError::InvalidFilter(e.to_string()))
}

/// Remove our flag only before the command-line separator. Preserve argv[0].
pub fn strip_args(args: &mut Vec<String>) -> Result<Option<Level>, LogError> {
    let mut level = None;
    let mut options = true;
    let mut kept = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        if i > 0 && arg == "--" {
            options = false;
        }
        if i > 0 && options && arg == "--debug" {
            level = Some(Level::Debug);
        } else if i > 0
            && options
            && let Some(value) = arg.strip_prefix("--debug=")
        {
            level = Some(Level::parse(value)?);
        } else {
            kept.push(arg.clone());
        }
    }
    *args = kept;
    Ok(level)
}

fn baseline(off_filter: &str, rust_log: Option<String>) -> String {
    rust_log
        .filter(|filter| !filter.is_empty())
        .unwrap_or_else(|| off_filter.to_owned())
}

pub(crate) fn stderr_mode(tty: bool, journal_active: bool) -> TriState {
    if tty || !journal_active {
        TriState::Always
    } else {
        TriState::Never
    }
}

/// Call first in main and retain the returned guard until process exit.
/// RUST_LOG is recorded once; enabling debug replaces it, disabling restores it.
pub fn init(
    identity: &str,
    off_filter: &str,
    args: &mut Vec<String>,
) -> Result<LogHandle, LogError> {
    let level = strip_args(args)?;
    // An invalid app default is a programming error. A bad environment
    // override must not prevent a GUI from starting or disable its live controls.
    parse_filter(off_filter)?;
    let mut baseline = baseline(off_filter, std::env::var("RUST_LOG").ok());
    if let Err(error) = parse_filter(&baseline) {
        eprintln!("{identity}: invalid RUST_LOG ({error}); using {off_filter}");
        baseline = off_filter.to_owned();
    }
    let filter = level.map(filter_for).unwrap_or_else(|| baseline.clone());
    let guard = crate::init::init_inner(
        &LogOpts {
            log_stderr: Some(stderr_mode(std::io::stderr().is_terminal(), true)),
            ..LogOpts::default()
        },
        &StatsOpts::default(),
        LogDefaults::gui(identity).with_journald(true),
        Some(parse_filter(&filter)?),
    )?;
    CONTROL
        .set(Mutex::new(Control {
            reload: guard
                .reload_handle()
                .expect("logging always installs a reload handle"),
            baseline,
            filter,
        }))
        .map_err(|_| LogError::AlreadyInstalled("debug control already installed".into()))?;
    tracing::debug!(identity, "debug logging initialised");
    Ok(guard)
}

/// Exactly one of `level` and the compatibility shorthand `on` is required.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetRequest {
    pub level: Option<Level>,
    pub on: Option<bool>,
}

pub fn describe(service: &str) -> Vec<Value> {
    vec![
        json!({"name":format!("{service}.debug"),"read_only":true,"description":"Read live debug state","params":{}}),
        json!({"name":format!("{service}.debug.set"),"read_only":false,"description":"Set level or on shorthand; exactly one required","args":{"level":"optional off|error|warn|info|debug|trace","on":"optional boolean; true=debug, false=baseline (exactly one of level/on)"},"params":{"level":{"type":"string","enum":["off","error","warn","info","debug","trace"]},"on":{"type":"boolean"}}}),
    ]
}

/// Intercept this service's debug family; other commands remain with the owner.
pub fn request(service: &str, command: &str, body: &str) -> Option<(u8, Value)> {
    let query = command.strip_suffix(".debug") == Some(service);
    if !query && command.strip_suffix(".debug.set") != Some(service) {
        return None;
    }
    Some(answer(query, body))
}

/// Preserve the app's discovery names while also serving an instance's name.
pub fn request_for(service: &str, app: &str, command: &str, body: &str) -> Option<(u8, Value)> {
    request(service, command, body).or_else(|| {
        if service == app {
            None
        } else {
            request(app, command, body)
        }
    })
}

fn answer(query: bool, body: &str) -> (u8, Value) {
    let result = (|| -> Result<Value, String> {
        if body.len() > 8192 {
            return Err("request exceeds 8192 bytes".into());
        }
        let body = if body.trim().is_empty() { "{}" } else { body };
        let request = if query {
            let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
            if !value.as_object().is_some_and(|v| v.is_empty()) {
                return Err("debug query accepts an empty object".into());
            }
            None
        } else {
            let request = serde_json::from_str::<SetRequest>(body).map_err(|e| e.to_string())?;
            let shape: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
            if !shape.as_object().is_some_and(|fields| fields.len() == 1) {
                return Err("provide an object with exactly one of level or on".into());
            }
            Some(request)
        };
        let mut control = CONTROL
            .get()
            .ok_or("debug logging is not initialised")?
            .lock()
            .map_err(|e| e.to_string())?;
        match request {
            Some(request) => control.set(request).map_err(|e| e.to_string()),
            None => Ok(control.state()),
        }
    })();
    match result {
        Ok(value) => (0, value),
        Err(error) => (10, json!({"error_code":"INVALID_ARGUMENT","message":error})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    #[test]
    fn workspace_targets_cover_packages_and_library_names() {
        fn check(path: &std::path::Path) {
            if let Ok(manifest) = std::fs::read_to_string(path.join("Cargo.toml")) {
                let mut section = "";
                for line in manifest.lines().map(str::trim) {
                    if line.starts_with('[') {
                        section = line;
                    }
                    if matches!(section, "[package]" | "[lib]")
                        && let Some((key, value)) = line.split_once('=')
                        && key.trim() == "name"
                    {
                        let name = value.trim().trim_matches('"').replace('-', "_");
                        assert!(
                            WORKSPACE_TARGETS.contains(&name.as_str()),
                            "missing own target {name} in {}",
                            path.display()
                        );
                    }
                }
            }
            for entry in std::fs::read_dir(path).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() && entry.file_name() != "target" {
                    check(&entry.path());
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for kind in ["apps", "libs", "services", "cli"] {
            check(&root.join(kind));
        }
        for line in include_str!("../../../Cargo.lock").lines() {
            if let Some(name) = line
                .strip_prefix("name = \"")
                .and_then(|name| name.strip_suffix('"'))
            {
                let target = name.replace('-', "_");
                if !WORKSPACE_TARGETS.contains(&target.as_str())
                    && WORKSPACE_TARGETS.iter().any(|own| target.starts_with(*own))
                {
                    assert!(
                        PREFIX_DEPENDENCIES.contains(&target.as_str()),
                        "dependency prefix needs an explicit cap: {target}"
                    );
                }
            }
        }
    }

    #[test]
    fn disabled_events_do_not_evaluate_fields() {
        let subscriber = tracing_subscriber::registry().with(parse_filter("warn").unwrap());
        tracing::subscriber::with_default(subscriber, || {
            let evaluated = std::cell::Cell::new(false);
            tracing::debug!(
                value = {
                    evaluated.set(true);
                    1
                },
                "disabled seam"
            );
            assert!(!evaluated.get());
        });
    }

    #[test]
    fn flags_stop_at_separator() {
        let mut args = [
            "term",
            "--debug",
            "--debug=trace",
            "--",
            "echo",
            "--debug=bad",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(strip_args(&mut args).unwrap(), Some(Level::Trace));
        assert_eq!(args, ["term", "--", "echo", "--debug=bad"]);
        assert_eq!(strip_args(&mut args).unwrap(), None);
        for value in ["", "DEBUG", "yes"] {
            assert!(strip_args(&mut vec!["app".into(), format!("--debug={value}")]).is_err());
        }
    }

    #[test]
    fn baseline_and_sink_rules() {
        assert_eq!(baseline("warn,email=info", None), "warn,email=info");
        assert_eq!(baseline("warn", Some("off".into())), "off");
        assert_eq!(baseline("warn", Some(String::new())), "warn");
        assert_eq!(stderr_mode(true, true), TriState::Always);
        assert_eq!(stderr_mode(false, true), TriState::Never);
        assert_eq!(stderr_mode(false, false), TriState::Always);
    }

    #[test]
    fn filters_enforce_own_dependency_and_clamped_levels() {
        for level in [
            Level::Off,
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            let text = filter_for(level);
            let subscriber = tracing_subscriber::registry().with(parse_filter(&text).unwrap());
            tracing::subscriber::with_default(subscriber, || {
                assert_eq!(
                    tracing::enabled!(target: "toolkit", tracing::Level::DEBUG),
                    matches!(level, Level::Debug | Level::Trace)
                );
                assert_eq!(
                    tracing::enabled!(target: "world", tracing::Level::TRACE),
                    level == Level::Trace
                );
                assert!(!tracing::enabled!(target: "external", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "strict_num", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "system_deps", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "termcolor", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "wgpu_core", tracing::Level::INFO));
                assert_eq!(
                    tracing::enabled!(target: "tokio", tracing::Level::WARN),
                    !matches!(level, Level::Off | Level::Error)
                );
                assert_eq!(
                    tracing::enabled!(target: "external", tracing::Level::ERROR),
                    level != Level::Off
                );
            });
        }
    }

    #[test]
    fn set_query_round_trip_replaces_filter_and_rejects_bad_input() {
        let (layer, inner) =
            tracing_subscriber::reload::Layer::new(parse_filter("warn,email=info").unwrap());
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);
        CONTROL
            .set(Mutex::new(Control {
                reload: LogReloadHandle { inner },
                baseline: "warn,email=info".into(),
                filter: "warn,email=info".into(),
            }))
            .ok()
            .unwrap();
        assert_eq!(answer(true, "{}").1["on"], false);
        assert_eq!(
            answer(false, r#"{"on":true}"#).1["filter"],
            filter_for(Level::Debug)
        );
        assert_eq!(answer(true, "{}").1["on"], true);
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            assert_eq!(answer(false, &json!({"level":level}).to_string()).0, 0);
            assert_eq!(answer(true, "{}").1["level"], level);
            assert_eq!(
                answer(true, "{}").1["on"],
                matches!(level, "debug" | "trace")
            );
        }
        assert_eq!(
            answer(false, r#"{"on":false}"#).1["filter"],
            "warn,email=info"
        );
        for body in [
            "{}",
            r#"{"on":"yes"}"#,
            r#"{"on":true,"extra":0}"#,
            r#"{"level":"bad"}"#,
            r#"{"level":"warn","on":true}"#,
            r#"{"level":"warn","on":null}"#,
            r#"["debug",null]"#,
        ] {
            assert_eq!(answer(false, body).0, 10);
        }
        assert_eq!(answer(true, r#"{"on":true}"#).0, 10);
        assert!(request("other", "example.debug", "{}").is_none());
    }
}
