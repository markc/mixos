// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `inputd`, the input broker: keys become Bus commands.
//!
//! It registers the `inputd` Bus service and answers the `input.*` commands
//! over the shared [`Resolver`], with key and pointer injection through a
//! uinput device of its own; neither needs a physical keyboard. `--observe`
//! reads a keyboard without grabbing it and logs what each key would do.
//! `--grab` and `--grab-name` are the interception path: the keyboard is
//! grabbed, the person's bindings fire, the user keys they left unbound are
//! swallowed, and everything else is re-emitted on `inputd virtual
//! keyboard`. `--vt` limits the grab to the listed VTs.

mod devices;
mod grab;
mod inject;
mod keymap_file;
mod reader;
mod service;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bus::native_client::{ConnState, SupervisedClient};
use clap::Parser;
use inputd_core::Resolver;
use inputd_schema::{InputKeymap, KEYMAP_SCHEMA_VERSION, SERVICE};

use service::Shared;

#[derive(Parser, Debug)]
#[command(
    name = "inputd",
    about = "Input broker: keys become Bus commands. Serves the inputd service; --observe \
             reads a keyboard safely; --grab or --grab-name intercepts it, optionally \
             only on the VTs given with --vt."
)]
struct Args {
    /// Read this keyboard event node without grabbing it: nothing is
    /// re-emitted and nothing fires. Needs root, and reads keystrokes.
    #[arg(long, value_name = "DEVICE", conflicts_with_all = ["grab", "grab_name"])]
    observe: Option<String>,

    /// Grab this keyboard event node: bound strokes fire their commands,
    /// unbound user keys are swallowed, the rest is re-emitted. Killing the
    /// process releases the grab.
    #[arg(long, value_name = "DEVICE")]
    grab: Option<String>,

    /// Like --grab, but find the node by its name in /proc/bus/input/devices
    /// (for example "keyd virtual keyboard"); the eventN shifts across
    /// restarts, so a unit should use this.
    #[arg(long, value_name = "NAME", conflicts_with = "grab")]
    grab_name: Option<String>,

    /// Hold the grab only while one of these VTs is active
    /// (/sys/class/tty/tty0/active), released on every switch away.
    /// Without it the grab is held always.
    #[arg(long, value_name = "N[,M...]", value_delimiter = ',', num_args = 1..)]
    vt: Option<Vec<u16>>,

    /// Exit after this many seconds of grabbing, which releases the grab.
    /// For the first supervised run.
    /// At least 1: 0 would exit before the first grab.
    #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
    grab_timeout: Option<u64>,

    /// The keymap file to load and write rebinds to. Default: keymap.json
    /// in $STATE_DIRECTORY, else mixos:var/inputd/keymap.json (root)
    /// or ~/.local/share/mixos/inputd/keymap.json.
    #[arg(long, value_name = "PATH")]
    keymap: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();
    let args = Args::parse();

    let grabbing = args.grab.is_some() || args.grab_name.is_some();
    if args.vt.is_some() && !grabbing {
        anyhow::bail!("--vt applies to the grab: give --grab or --grab-name too");
    }
    let vts = args
        .vt
        .as_deref()
        .map(grab::parse_vts)
        .transpose()
        .map_err(anyhow::Error::msg)?;

    // The keymap file's rows if it loads; otherwise the defaults, seeded so
    // the file exists. An unusable document is moved aside, never deleted
    // (keymap_file::open). The path is kept even when writing is off, so
    // replies stay truthful and input.reload can turn writing back on.
    let path = args.keymap.unwrap_or_else(keymap_file::default_path);
    let opened = keymap_file::open(&path, &keymap_file::local_stamp());
    let keymap = InputKeymap {
        version: KEYMAP_SCHEMA_VERSION,
        physical: opened.rows,
    };
    let mesh_open = service::mesh_open_from_env();
    tracing::info!(
        mesh_open,
        "mesh access {}",
        if mesh_open {
            "open: every command reachable by mesh callers"
        } else {
            "locked (MIXOS_MESH_OPEN=0): keymap changes node-local only"
        }
    );
    let store = service::Store::new(Some(path), opened.recovered_from, opened.persist_disabled)
        .with_mesh_open(mesh_open);
    let resolver: Shared = Arc::new(Mutex::new(Resolver::new(keymap)));

    // The reader thread sends resolved commands to the Bus side. The original
    // sender lives as long as the process, so the receiver never closes.
    let (fire_tx, fire_rx) = tokio::sync::mpsc::unbounded_channel::<reader::FiredCommand>();

    let grab_device = match (&args.grab, &args.grab_name) {
        (Some(path), _) => Some(path.clone()),
        (None, Some(name)) => {
            let devices = std::fs::read_to_string("/proc/bus/input/devices")?;
            let path = devices::device_by_name(&devices, name).ok_or_else(|| {
                anyhow::anyhow!("no input device named {name:?} in /proc/bus/input/devices")
            })?;
            tracing::info!(%name, %path, "grab device found by name");
            Some(path)
        }
        (None, None) => None,
    };
    if let Some(device) = grab_device {
        let options = reader::GrabOptions {
            device,
            vts,
            timeout: args.grab_timeout.map(Duration::from_secs),
        };
        let (shared, tx) = (Arc::clone(&resolver), fire_tx.clone());
        std::thread::spawn(move || {
            // run_grab returns only on an error. The kernel has released
            // the grab with the fd; exit so systemd starts a fresh broker
            // rather than leaving one with no keyboard. A panic exits too:
            // a broker whose reader died must not stay up looking healthy.
            if let Err((status, error)) = reader_outcome(|| reader::run_grab(options, shared, tx)) {
                tracing::error!(%error, "grab reader stopped");
                std::process::exit(status);
            }
        });
    } else if let Some(device) = args.observe {
        let shared = Arc::clone(&resolver);
        std::thread::spawn(move || {
            match reader_outcome(|| reader::run_observe(&device, shared)) {
                Ok(()) => {}
                // An observe error leaves the Bus service up, as before.
                Err((1, error)) => tracing::error!(%error, "observe reader stopped"),
                Err((status, error)) => {
                    tracing::error!(%error, "observe reader panicked");
                    std::process::exit(status);
                }
            }
        });
    }
    let _fire_tx = fire_tx;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve(resolver, store, fire_rx))
}

/// Run a reader body on its thread. `Err((1, why))` when it returned an
/// error, `Err((101, why))` when it panicked (the panic itself is also
/// printed by the default hook).
fn reader_outcome(body: impl FnOnce() -> std::io::Result<()>) -> Result<(), (i32, String)> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err((1, error.to_string())),
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            Err((101, format!("panicked: {why}")))
        }
    }
}

/// Serve `input.*` and send fired commands, across broker restarts.
async fn serve(
    resolver: Shared,
    store: service::Store,
    mut fire_rx: tokio::sync::mpsc::UnboundedReceiver<reader::FiredCommand>,
) -> anyhow::Result<()> {
    let build = buildinfo::build_info!();
    let provenance = bus::RegisterProvenance::from_parts(
        build.pkg,
        build.version,
        build.git_sha,
        build.git_dirty,
        build.build_time,
        buildinfo::now_rfc3339(),
    );
    let client =
        SupervisedClient::connect_options(SERVICE, &bus::client_helpers::resolve_noded_url())
            .fatal_on_registration_rejection(true)
            .with_commands(service::command_manifest())
            .with_provenance(provenance)
            .start();
    let mut incoming = client
        .incoming()
        .ok_or_else(|| anyhow::anyhow!("incoming stream already taken"))?;
    let mut state = client.subscribe_state();
    // The injector outlives broker reconnects.
    let mut injector = inject::Injector::default();
    loop {
        tokio::select! {
            changed = state.changed() => {
                if changed.is_err() {
                    break;
                }
                match *state.borrow_and_update() {
                    ConnState::Fatal => anyhow::bail!("Bus registration rejected: another inputd holds the name"),
                    ConnState::Connected => tracing::info!("registered as {SERVICE}; serving input.*"),
                    _ => {}
                }
            }
            command = incoming.recv() => {
                let Some(command) = command else { break };
                if command.is_topic_delivery() {
                    continue;
                }
                let (rc, body) = service::dispatch(&resolver, &store, &mut injector, &command);
                if let Err(error) = client.respond(&command, rc, &body).await {
                    tracing::warn!(%error, command_name = %command.command, "reply failed");
                }
            }
            fired = fire_rx.recv() => {
                let Some(fired) = fired else { break };
                fire_command(&client, &fired).await;
            }
        }
    }
    Ok(())
}

/// The `(service, command)` a fired command is sent as: the row's explicit
/// `service` with the command unchanged, else the command's first dot-segment
/// (`desktop.workspace.next` -> `desktop`). `None` when there is no target.
fn fire_target(fired: &reader::FiredCommand) -> Option<(&str, &str)> {
    let service = match fired.service.as_deref() {
        Some(explicit) => explicit,
        None => fired.command.split('.').next().unwrap_or(""),
    };
    (!service.is_empty()).then_some((service, fired.command.as_str()))
}

/// Send a fired command, fire-and-forget, with its args as the body. A command
/// nobody answers simply does not route.
async fn fire_command(client: &SupervisedClient, fired: &reader::FiredCommand) {
    let Some((service, command)) = fire_target(fired) else {
        return;
    };
    // Whatever admitted the row, a non-map never reaches the wire.
    let body = match fired.args.clone() {
        Some(args) if args.is_object() => args,
        Some(_) => {
            tracing::warn!(command_name = %fired.command, "non-object args dropped");
            serde_json::json!({})
        }
        None => serde_json::json!({}),
    };
    if let Err(error) = client.send(service, command, body).await {
        tracing::warn!(%error, command, service, "fire failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inputd_core::{Edge, SCENE_EDITOR_CHORDS, SCENE_EDITOR_KEY, default_keymap};

    fn fired(command_name: &str, service: Option<&str>) -> reader::FiredCommand {
        reader::FiredCommand {
            command: command_name.to_string(),
            args: None,
            service: service.map(str::to_string),
        }
    }

    #[test]
    fn an_explicit_service_is_the_target_and_the_command_is_unchanged() {
        let f = fired("desktop.clipboard.menu", Some("desktop-vt1"));
        assert_eq!(
            fire_target(&f),
            Some(("desktop-vt1", "desktop.clipboard.menu"))
        );
    }

    #[test]
    fn an_absent_service_routes_by_first_segment() {
        let f = fired("desktop.workspace.next", None);
        assert_eq!(fire_target(&f), Some(("desktop", "desktop.workspace.next")));
    }

    #[test]
    fn no_derivable_target_fires_nothing() {
        assert_eq!(fire_target(&fired(".x", None)), None);
        assert_eq!(fire_target(&fired("desktop.x", Some(""))), None);
    }

    #[test]
    fn the_scene_editor_chord_fires_a_safe_open_at_the_loader() {
        let r = Resolver::new(default_keymap());
        for modifiers in SCENE_EDITOR_CHORDS {
            let out = r.resolve(SCENE_EDITOR_KEY, modifiers, Edge::Press);
            let f = reader::FiredCommand {
                command: out.command.expect("bound").as_str().to_string(),
                args: out.args,
                service: out.service,
            };
            assert_eq!(fire_target(&f), Some(("scenes", "scenes.editor.open")));
            assert_eq!(f.args, Some(serde_json::json!({"safe": true})));
        }
    }

    #[test]
    fn a_reader_panic_or_error_becomes_an_exit_status() {
        assert_eq!(reader_outcome(|| Ok(())), Ok(()));
        let (status, why) =
            reader_outcome(|| Err(std::io::Error::other("device went away"))).unwrap_err();
        assert_eq!((status, why.as_str()), (1, "device went away"));
        let (status, why) = reader_outcome(|| panic!("latch table corrupt")).unwrap_err();
        assert_eq!(status, 101);
        assert!(why.contains("latch table corrupt"), "{why}");
        let owned = String::from("formatted panic");
        let (status, why) = reader_outcome(move || panic!("{owned}")).unwrap_err();
        assert_eq!(status, 101);
        assert!(why.contains("formatted panic"), "{why}");
    }

    #[test]
    fn a_zero_grab_timeout_is_refused() {
        assert!(
            Args::try_parse_from([
                "inputd",
                "--grab",
                "/dev/input/event1",
                "--grab-timeout",
                "0"
            ])
            .is_err()
        );
        let args = Args::try_parse_from([
            "inputd",
            "--grab",
            "/dev/input/event1",
            "--grab-timeout",
            "30",
        ])
        .unwrap();
        assert_eq!(args.grab_timeout, Some(30));
    }

    #[test]
    fn the_cli_takes_a_vt_list_and_refuses_bad_combinations() {
        let args = Args::try_parse_from([
            "inputd",
            "--grab-name",
            "keyd virtual keyboard",
            "--vt",
            "1,4",
        ])
        .unwrap();
        assert_eq!(args.vt, Some(vec![1, 4]));
        assert!(
            Args::try_parse_from(["inputd", "--grab", "/dev/input/event1", "--grab-name", "x"])
                .is_err()
        );
        assert!(
            Args::try_parse_from(["inputd", "--observe", "/dev/input/event1", "--grab", "x"])
                .is_err()
        );
        let plain = Args::try_parse_from(["inputd"]).unwrap();
        assert!(plain.vt.is_none() && plain.grab.is_none() && plain.keymap.is_none());
    }
}
