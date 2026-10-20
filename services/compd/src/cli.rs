// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Command line: backend selection plus the flags the desktop gates pass.
//!
//! The desktop gates launch the compositor as
//! `compd --nested --socket S [--bus-service N]` and
//! `compd kms-live --device D --connector C --scale S`.
//! compd accepts the flags that map onto something it really does, and refuses
//! the rest by name, so a gate never silently runs the wrong compositor.
//!
//! | flag | compd |
//! |---|---|
//! | `--nested` | nested (winit) backend |
//! | `--headless` | render node only, virtual `headless-0` output, no seat or XWayland |
//! | `--size WxH` | headless physical mode (default 1920x1080 at 60 Hz) |
//! | `--render-node PATH` | headless render node (default `render_node` setting) |
//! | `--kms`, `kms-live` | KMS backend (`kms-live` is the subcommand spelling) |
//! | (neither) | both compiled in: nested when `WAYLAND_DISPLAY`/`DISPLAY` is set, else KMS |
//! | `--socket NAME` | the Wayland socket name (default: the next free `wayland-N`) |
//! | `--device PATH` | KMS: the scanout DRM node (overrides `scanout_node`) |
//! | `--connector NAME` | KMS: drive this connector (`HDMI-A-1`, `eDP-1`, …) |
//! | `--scale S` | the output scale, 0.5 ≤ S ≤ 4, rounded to 1/120 (wp_fractional_scale's unit); KMS/headless default to 1, nested follows the host |
//! | `--kms-confirm` | accepted, no effect: compd never asks a human to confirm a takeover |
//! | `--config-file PATH` | the settings file (read by `model::environment::config`) |
//! | `--bus-service NAME` | the `comp` Bus service name (default `comp` on KMS, `comp-nested` nested, `comp-headless` headless) |
//! | `--scene-service NAME` | the Mix Scenes host's fallback Bus name when `shell` is taken (overrides the `scene_service` preference; the host runs only with `scene_host` on) |
//! | `--version`, `-V` / `--help`, `-h` | print and exit 0 |
//! | `--chrome`, anything else | refused, exit 2 |

use std::process::exit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Nested,
    Kms,
    Headless,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Nested => "nested",
            Backend::Kms => "kms",
            Backend::Headless => "headless",
        }
    }
}

#[derive(Debug, Default)]
pub struct Cli {
    /// Explicitly requested backend, if any.
    pub backend: Option<Backend>,
    pub socket: Option<String>,
    pub device: Option<String>,
    pub connector: Option<String>,
    pub size: Option<(i32, i32)>,
    pub render_node: Option<String>,
    /// `--bus-service`: the Bus service name, already checked against the ABP
    /// name grammar.
    pub bus_service: Option<String>,
    /// `--scene-service`: the scene host's fallback name, checked the same way.
    pub scene_service: Option<String>,
    /// `--scale`: the requested output scale, already range-checked and
    /// rounded to 1/120. `None` is the backend default ([`output_scale`]).
    pub scale: Option<f64>,
}

const USAGE: &str = "usage: compd [--nested | --kms | kms-live | --headless] [--socket NAME] \
[--device PATH] [--connector NAME] [--size WxH] [--render-node PATH] [--scale S] [--kms-confirm] [--config-file PATH] \
[--bus-service NAME] [--scene-service NAME] [--version] [--help]";

fn refuse(msg: &str) -> ! {
    eprintln!("compd: {msg}");
    eprintln!("{USAGE}");
    exit(2);
}

/// Parse `std::env::args`. Prints and exits for `--help`/`--version` and for every
/// refused or malformed flag; never returns an error. `version` is the buildinfo
/// line (`compd <semver> (<sha12>, built <ts>)`).
pub fn parse(version: &str) -> Cli {
    parse_from(std::env::args().skip(1), version)
}

pub fn parse_from(args: impl Iterator<Item = String>, version: &str) -> Cli {
    let mut cli = Cli::default();
    let mut args = args.peekable();
    let mut set_backend = |cli: &mut Cli, b: Backend| {
        if cli.backend.is_some_and(|prev| prev != b) {
            refuse("--nested, --kms/kms-live and --headless are mutually exclusive");
        }
        cli.backend = Some(b);
    };
    while let Some(arg) = args.next() {
        // `--flag=value` and `--flag value` are both accepted for valued flags.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> String {
            inline
                .clone()
                .or_else(|| args.next())
                .unwrap_or_else(|| refuse(&format!("{name} needs a value")))
        };
        match flag.as_str() {
            "--help" | "-h" => {
                println!("{version}\n{USAGE}");
                exit(0);
            }
            // Answered first by `exit_on_version!` in main; kept so the parser
            // never rejects the flag.
            "--version" | "-V" => {
                println!("{version}");
                exit(0);
            }
            "--nested" => set_backend(&mut cli, Backend::Nested),
            "--headless" => set_backend(&mut cli, Backend::Headless),
            "--render-node" => cli.render_node = Some(value("--render-node")),
            "--size" => {
                let v = value("--size");
                cli.size = Some(parse_size(&v).unwrap_or_else(|why| refuse(&format!("--size {v}: {why}"))));
            }
            "--kms" | "kms-live" => set_backend(&mut cli, Backend::Kms),
            "--socket" => cli.socket = Some(value("--socket")),
            "--device" => cli.device = Some(value("--device")),
            "--connector" => cli.connector = Some(value("--connector")),
            "--config-file" => {
                // Read by the config layer from argv itself; only consumed here.
                let _ = value("--config-file");
            }
            "--scale" => {
                let v = value("--scale");
                match parse_scale(&v) {
                    Ok(scale) => cli.scale = Some(scale),
                    Err(why) => refuse(&format!("--scale {v}: {why}")),
                }
            }
            "--kms-confirm" => {
                // An opt-in human gate on a KMS takeover. compd never asks
                // (agentic-first: unattended is the default), so it is a no-op.
            }
            "--chrome" => refuse(
                "--chrome: compd draws its own window decorations; there are no \
                 selectable chrome styles",
            ),
            "--bus-service" => {
                let name = value("--bus-service");
                if let Err(why) = comp_service::port::validate_service_name(&name) {
                    refuse(&format!("--bus-service: {why}"));
                }
                cli.bus_service = Some(name);
            }
            "--scene-service" => {
                let name = value("--scene-service");
                if let Err(why) = comp_service::port::validate_service_name(&name) {
                    refuse(&format!("--scene-service: {why}"));
                }
                cli.scene_service = Some(name);
            }
            other => refuse(&format!("unknown argument {other:?}")),
        }
    }
    // KMS-only flags imply KMS when no backend was named.
    if cli.backend.is_none() && (cli.device.is_some() || cli.connector.is_some()) {
        cli.backend = Some(Backend::Kms);
    }
    if cli.backend != Some(Backend::Kms) && (cli.device.is_some() || cli.connector.is_some()) {
        refuse("--device/--connector apply only to the KMS backend");
    }
    if cli.backend != Some(Backend::Headless) && (cli.size.is_some() || cli.render_node.is_some()) {
        refuse("--size/--render-node require --headless");
    }
    cli
}

/// The smallest and largest output scale compd accepts.
pub const SCALE_MIN: f64 = 0.5;
pub const SCALE_MAX: f64 = 4.0;

/// Physical output dimensions; reject empty, overflowing and zero-sized modes.
pub fn parse_size(value: &str) -> Result<(i32, i32), &'static str> {
    let (w, h) = value.split_once('x').ok_or("expected WxH")?;
    let w: i32 = w.parse().map_err(|_| "invalid width")?;
    let h: i32 = h.parse().map_err(|_| "invalid height")?;
    if w <= 0 || h <= 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)).is_none() {
        return Err("dimensions must be positive and fit a pixel buffer");
    }
    Ok((w, h))
}

/// `--scale`'s value: a finite number in [`SCALE_MIN`, `SCALE_MAX`], rounded
/// to 1/120 (the unit `wp_fractional_scale_v1` carries, so the scale a client
/// is told is exactly the one compd renders at).
pub fn parse_scale(value: &str) -> Result<f64, &'static str> {
    let scale: f64 = value.trim().parse().map_err(|_| "not a number")?;
    if !scale.is_finite() {
        return Err("not a finite number");
    }
    let rounded = (scale * 120.0).round() / 120.0;
    if !(SCALE_MIN..=SCALE_MAX).contains(&rounded) {
        return Err("out of range (0.5 to 4)");
    }
    Ok(rounded)
}

/// The output scale a backend applies: the requested one, else 1 on KMS. A
/// nested run with no `--scale` returns `None`: it follows the host.
pub fn output_scale(cli: &Cli, backend: Backend) -> Option<f64> {
    match backend {
        Backend::Kms | Backend::Headless => Some(cli.scale.unwrap_or(1.0)),
        Backend::Nested => cli.scale,
    }
}

/// The backend this run uses: the requested one if it is compiled in, else the
/// only one compiled in, else (both compiled in) nested when a host display is
/// reachable. Refuses, naming the cargo feature, when the request is not built.
pub fn resolve(cli: &Cli) -> Backend {
    let nested_built = cfg!(feature = "backend-winit");
    let kms_built = cfg!(feature = "backend-native");
    let headless_built = cfg!(feature = "backend-headless");
    match cli.backend {
        Some(Backend::Headless) if !headless_built => {
            refuse("headless-feature-disabled: --headless needs the backend-headless feature")
        }
        Some(Backend::Nested) if !nested_built => {
            refuse("kms-live-only build: --nested needs the backend-winit feature")
        }
        // The reason code the deploy probe looks for.
        Some(Backend::Kms) if !kms_built => {
            refuse("kms-live-feature-disabled: this build lacks the backend-native feature")
        }
        Some(b) => b,
        None if nested_built && !kms_built => Backend::Nested,
        None if kms_built && !nested_built => Backend::Kms,
        None if headless_built && !nested_built && !kms_built => Backend::Headless,
        None => {
            let host = |v: &str| std::env::var_os(v).is_some_and(|s| !s.is_empty());
            if host("WAYLAND_DISPLAY") || host("DISPLAY") {
                Backend::Nested
            } else {
                Backend::Kms
            }
        }
    }
}

/// Environment knobs compd does not implement. Environment leaks in from parent
/// sessions far more often than flags, so these warn instead of refusing.
pub const UNSUPPORTED_ENV: &[&str] = &[
    "COMPD_CAPTURE_DIR",
    "COMPD_CAPTURE_EVERY",
    "COMPD_CAPTURE_ON_SIGNAL",
    "COMPD_QUOIN_PRIVATE_INTERACTION",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_launch_parses_mode_scale_and_node() {
        let cli = p(&["--headless", "--size=1280x720", "--scale", "2", "--render-node", "/dev/dri/renderD128"]);
        assert_eq!(cli.backend, Some(Backend::Headless));
        assert_eq!(cli.size, Some((1280, 720)));
        assert_eq!(cli.render_node.as_deref(), Some("/dev/dri/renderD128"));
        assert_eq!(output_scale(&cli, Backend::Headless), Some(2.0));
        assert_eq!(output_scale(&p(&["--headless"]), Backend::Headless), Some(1.0));
        assert_eq!(crate::comp::service_name(Backend::Headless, None), "comp-headless");
        for bad in ["", "0x720", "1280x0", "-1x720", "1280", "1280x720x2", "2147483647x2"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    fn p(args: &[&str]) -> Cli {
        parse_from(args.iter().map(|s| s.to_string()), "test")
    }

    #[test]
    fn comp_nested_launch_parses() {
        let c = p(&["--nested", "--socket", "wayland-77"]);
        assert_eq!(c.backend, Some(Backend::Nested));
        assert_eq!(c.socket.as_deref(), Some("wayland-77"));
    }

    #[test]
    fn bus_service_is_accepted_and_defaults_by_backend() {
        let c = p(&["--nested", "--bus-service", "compd-smoke-7"]);
        assert_eq!(c.bus_service.as_deref(), Some("compd-smoke-7"));
        assert_eq!(
            crate::comp::service_name(Backend::Nested, c.bus_service.as_deref()),
            "compd-smoke-7"
        );
        assert_eq!(
            crate::comp::service_name(Backend::Nested, None),
            "comp-nested"
        );
        assert_eq!(crate::comp::service_name(Backend::Kms, None), "comp");
    }

    #[test]
    fn scene_service_is_accepted_in_both_forms() {
        assert_eq!(
            p(&["--nested", "--scene-service", "shell-nested"])
                .scene_service
                .as_deref(),
            Some("shell-nested")
        );
        assert_eq!(
            p(&["--scene-service=shell-7"]).scene_service.as_deref(),
            Some("shell-7")
        );
        assert_eq!(p(&["--nested"]).scene_service, None);
    }

    #[test]
    fn comp_kms_live_launch_parses() {
        let c = p(&[
            "kms-live",
            "--device",
            "/dev/dri/card1",
            "--connector",
            "HDMI-A-1",
            "--scale",
            "1",
            "--kms-confirm",
        ]);
        assert_eq!(c.backend, Some(Backend::Kms));
        assert_eq!(c.device.as_deref(), Some("/dev/dri/card1"));
        assert_eq!(c.connector.as_deref(), Some("HDMI-A-1"));
        assert_eq!(c.scale, Some(1.0));
        assert_eq!(output_scale(&c, Backend::Kms), Some(1.0));
    }

    #[test]
    fn scale_is_ranged_rounded_and_defaults_by_backend() {
        assert_eq!(parse_scale("2.5"), Ok(2.5));
        assert_eq!(parse_scale(" 1.25 "), Ok(1.25));
        // Rounded to 1/120: 1.3333 is 160/120.
        assert_eq!(parse_scale("1.3333"), Ok(160.0 / 120.0));
        assert_eq!(parse_scale("0.5"), Ok(0.5));
        assert_eq!(parse_scale("4"), Ok(4.0));
        assert!(parse_scale("0.4").is_err());
        assert!(parse_scale("4.01").is_err());
        assert!(parse_scale("NaN").is_err());
        assert!(parse_scale("inf").is_err());
        assert!(parse_scale("two").is_err());
        let nested = p(&["--nested"]);
        assert_eq!(
            output_scale(&nested, Backend::Nested),
            None,
            "nested follows the host"
        );
        assert_eq!(
            output_scale(&nested, Backend::Kms),
            Some(1.0),
            "KMS defaults to 1"
        );
        let scaled = p(&["--nested", "--scale=2.5"]);
        assert_eq!(output_scale(&scaled, Backend::Nested), Some(2.5));
    }

    #[test]
    fn device_implies_kms_and_equals_form_works() {
        let c = p(&["--device=/dev/dri/card0", "--config-file=/tmp/s.json"]);
        assert_eq!(c.backend, Some(Backend::Kms));
        assert_eq!(c.device.as_deref(), Some("/dev/dri/card0"));
    }
}
