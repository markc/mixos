// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! backdrop: the MixOS desktop background. One surface per output on compd's
//! Background layer (`toolkit::layer`), drawn with egui: a solid colour or a
//! PNG/JPEG image or slideshow, from the document `backdrop.mx` (or the
//! session theme's colour when there is none). The command line sets the
//! default; the Bus (`backdrop.set`) replaces the document. `--probe` runs the
//! regression probe instead (`probe.rs`).

mod control;
mod probe;
mod program;

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use toolkit::layer::LayerOptions;
use wallpaper::{Config, Fill, Wallpaper};

const HELP: &str = "backdrop — the MixOS desktop background\n\
Usage: backdrop [OPTIONS]\n\
  --config PATH      the document to read and that backdrop.set stores\n\
                     (default: backdrop.mx in the app's config directory)\n\
  --colour #RRGGBB   the default: a solid colour, over the document's\n\
  --image FILE       the default: a PNG or JPEG, centre-cropped, over the\n\
                     document's (an absolute path, or relative to here)\n\
  --fit MODE         image sizing: fill (default), fit, centre, tile, stretch\n\
  --service NAME     register on the Bus as NAME (default backdrop)\n\
  --probe            the regression probe: surfaces, scale and input\n\
  --help             this text\n\
Bus: backdrop.ping, backdrop.state, backdrop.set, backdrop.next (docs/backdrop/README.md).";

/// The largest document read (strict data: a few kilobytes in practice).
const MAX_DOCUMENT: u64 = 64 * 1024;

#[derive(Debug, Default)]
struct Args {
    probe: bool,
    config: Option<PathBuf>,
    colour: Option<String>,
    image: Option<String>,
    fit: Option<wallpaper::fit::Fit>,
    service: Option<String>,
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe" => out.probe = true,
            "--config" => out.config = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--colour" => out.colour = Some(value(&mut args, &arg)?),
            "--image" => out.image = Some(value(&mut args, &arg)?),
            "--fit" => {
                let mode = value(&mut args, &arg)?;
                out.fit = Some(
                    serde_json::from_value(serde_json::Value::String(mode))
                        .map_err(|_| "--fit needs fill, fit, centre, tile or stretch")?,
                );
            }
            "--service" => out.service = Some(value(&mut args, &arg)?),
            "--help" | "-h" => {
                println!("{HELP}\n{}", toolkit::debug::HELP);
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other} (see --help)")),
        }
    }
    if out.probe
        && (out.config.is_some()
            || out.colour.is_some()
            || out.image.is_some()
            || out.fit.is_some()
            || out.service.is_some())
    {
        return Err("--probe runs alone: no wallpaper or service option goes with it".into());
    }
    Ok(out)
}

/// A Bus service name: 2 to 31 of a-z, 0-9 and '-', starting with a letter.
fn valid_service(name: &str) -> bool {
    (2..=31).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The document backdrop keeps when none is named: `backdrop.mx` in the app's
/// config directory.
fn default_store() -> Option<PathBuf> {
    config::AppDirs::resolve(control::APP).map(|dirs| dirs.config().join("backdrop.mx"))
}

/// A document read and validated. `required`: a missing file is an error.
fn load(path: &Path, required: bool) -> Result<Option<Config>, String> {
    let shown = path.display();
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok(None);
        }
        Err(error) => return Err(format!("{shown}: {error}")),
    };
    let meta = file.metadata().map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_DOCUMENT {
        return Err(format!(
            "{shown}: expected a regular document of at most {MAX_DOCUMENT} bytes"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{shown}: {e}"))?;
    if bytes.len() as u64 > MAX_DOCUMENT {
        return Err(format!("{shown}: over {MAX_DOCUMENT} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|e| format!("{shown}: {e}"))?;
    Config::from_text(&text)
        .map(Some)
        .map_err(|e| format!("{shown}: {e}"))
}

/// The default the command line names, validated as the document's own are.
fn cli_default(args: &Args) -> Result<Option<Wallpaper>, String> {
    if args.fit.is_some() && args.image.is_none() {
        return Err("--fit goes with --image".into());
    }
    let fill = match (&args.colour, &args.image) {
        (Some(_), Some(_)) => {
            return Err("--colour and --image each set the default: use one".into());
        }
        (Some(colour), None) => Fill {
            colour: Some(colour.clone()),
            ..Fill::default()
        },
        (None, Some(image)) => {
            let path = std::path::absolute(image).map_err(|e| format!("--image {image}: {e}"))?;
            Fill {
                image: Some(path.display().to_string()),
                fit: args.fit,
                ..Fill::default()
            }
        }
        (None, None) => return Ok(None),
    };
    Wallpaper::from_fill(&fill, "default")
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The configuration to start with, and where accepted documents are stored.
fn startup(args: &Args) -> Result<(Config, Option<PathBuf>), String> {
    let store = match &args.config {
        Some(path) => {
            Some(std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?)
        }
        None => default_store(),
    };
    let file = match &store {
        Some(path) => load(path, args.config.is_some())?,
        None => None,
    };
    let mut config = file.unwrap_or_default();
    if let Some(default) = cli_default(args)? {
        config.default = default;
    }
    Ok((revalidate(&config)?, store))
}

/// The configuration after command-line overrides, validated as a document
/// again: a default from the command line counts toward the same limits (four
/// distinct images) as a document's own.
fn revalidate(config: &Config) -> Result<Config, String> {
    Config::from_document(config.document()).map_err(|e| format!("command line: {e}"))
}

fn main() {
    let mut cli_args: Vec<String> = std::env::args().collect();
    let _logging =
        toolkit::debug::init("backdrop", "warn", &mut cli_args).unwrap_or_else(|error| {
            toolkit::debug::failure("backdrop", &error);
            std::process::exit(2);
        });
    buildinfo::exit_on_version!();
    let args = match parse(cli_args.into_iter().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            toolkit::debug::failure("backdrop", &error);
            std::process::exit(2);
        }
    };
    if args.probe {
        if let Err(error) =
            toolkit::layer::run(LayerOptions::background(control::APP), probe::Probe::new())
        {
            toolkit::debug::failure("backdrop", &error);
            std::process::exit(1);
        }
        return;
    }
    let (config, store) = match startup(&args) {
        Ok(started) => started,
        Err(error) => {
            toolkit::debug::failure("backdrop", &error);
            std::process::exit(1);
        }
    };
    let service = args
        .service
        .clone()
        .unwrap_or_else(|| control::APP.to_owned());
    if !valid_service(&service) {
        eprintln!(
            "backdrop: --service {service:?}: 2 to 31 of a-z, 0-9 and '-', starting with a letter"
        );
        std::process::exit(2);
    }
    let shared = Arc::new(Mutex::new(program::Shared::default()));
    let wakers = Arc::new(Mutex::new(Vec::new()));
    let setup = control::Setup {
        service,
        url: citizen::noded_url(),
        store,
        config,
        shared: shared.clone(),
        wakers: wakers.clone(),
    };
    if let Err(error) = control::start(setup) {
        toolkit::debug::failure("backdrop", &error);
        std::process::exit(1);
    }
    let program = program::Backdrop::new(toolkit::Theme::load(), shared, wakers);
    if let Err(error) = toolkit::layer::run(LayerOptions::background(control::APP), program) {
        toolkit::debug::failure("backdrop", &error);
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
    fn the_command_line_is_strict() {
        assert!(args(&["--colour"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(args(&["--probe", "--colour", "#000000"]).is_err());
        let a = args(&["--colour", "#102030", "--service", "backdrop-test"]).unwrap();
        assert_eq!(a.colour.as_deref(), Some("#102030"));
        assert!(args(&["--probe"]).unwrap().probe);
    }

    #[test]
    fn the_service_name_follows_the_bus_rule() {
        assert!(valid_service("backdrop"));
        assert!(valid_service("backdrop-test-1"));
        for bad in ["", "a", "Backdrop", "1backdrop", "back.drop", "back drop"] {
            assert!(!valid_service(bad), "{bad:?}");
        }
    }

    #[test]
    fn command_line_fit_is_validated_and_requires_an_image() {
        assert!(args(&["--fit", "bogus"]).is_err());
        assert!(args(&["--probe", "--fit", "tile"]).is_err());
        let only_fit = args(&["--fit", "centre"]).unwrap();
        assert!(cli_default(&only_fit).is_err());
        let with_image = args(&["--image", "/a.png", "--fit", "fit"]).unwrap();
        assert!(matches!(
            cli_default(&with_image).unwrap(),
            Some(Wallpaper::Image {
                fit: wallpaper::fit::Fit::Fit,
                ..
            })
        ));
    }

    #[test]
    fn a_command_line_image_counts_toward_the_distinct_image_cap() {
        let four = Config::from_text(
            "schema: 1\noutputs: { a: { image: \"/w/1.png\" }, b: { image: \"/w/2.png\" }, c: { image: \"/w/3.png\" }, d: { image: \"/w/4.png\" } }",
        )
        .unwrap();
        assert!(revalidate(&four).is_ok());
        let image = |path: &str| {
            Wallpaper::from_fill(
                &Fill {
                    image: Some(path.into()),
                    ..Fill::default()
                },
                "default",
            )
            .unwrap()
        };
        let mut fifth = four.clone();
        fifth.default = image("/w/5.png");
        let error = revalidate(&fifth).unwrap_err();
        assert!(error.contains("distinct images"), "{error}");
        let mut shared = four.clone();
        shared.default = image("/w/1.png");
        assert!(revalidate(&shared).is_ok(), "a repeated path counts once");
    }

    #[test]
    fn a_command_line_default_is_validated_like_a_document() {
        let colour = Args {
            colour: Some("#0a0b0c".into()),
            ..Args::default()
        };
        assert_eq!(
            cli_default(&colour).unwrap(),
            Some(Wallpaper::Colour(wallpaper::Rgb([10, 11, 12])))
        );
        let bad = Args {
            colour: Some("red".into()),
            ..Args::default()
        };
        assert!(cli_default(&bad).is_err());
        let both = Args {
            colour: Some("#000000".into()),
            image: Some("/a.png".into()),
            ..Args::default()
        };
        assert!(cli_default(&both).is_err());
        assert_eq!(cli_default(&Args::default()).unwrap(), None);
    }
}
