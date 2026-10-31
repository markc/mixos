// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Headless dopus: the same core and Bus surface, no window. Neighbours can
//! read `dopus.state` (two panes) and drive `dopus.execute` against it. The
//! window and this loop share [`commands::serve_command`]; only the transport
//! differs.
//!
//! A drainer thread owns `on_event` and `tick` (laws 1 and 2) and retains
//! dialogs for typed Bus answers (law 3). `OpenFile` is refused and logged
//! (law 4). Theme and
//! window actions are refused: there is nothing to paint. Accepted
//! `dopus.execute` runs are published on `dopus.action.executed`, origin
//! `bus`, as the window publishes them.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::app::App;
use dopus_core::{ConfigFile, CoreEvent, DOpusConfig, DopusCore};
use futures::StreamExt;
use toolkit::action::Origin;

use crate::bus::{self, Delivery};
use crate::commands::{self, Served, ServerMeta};
use crate::dirs::AppDirs;

/// The drain cadence: how long a worker reply can wait, and the tick period.
const DRAIN_TICK: Duration = Duration::from_millis(200);

/// Retain prompts for typed answers; refuse external file launches.
pub fn answer_derived(app: &mut App, events: Vec<CoreEvent>, log: impl Fn(String)) {
    for event in events {
        match event {
            CoreEvent::OpenFile(path) => log(format!(
                "refusing OpenFile({}), headless never spawns",
                dopus_core::sanitise_display_path(&path)
            )),
            event => app.absorb(vec![event]),
        }
    }
}

fn lock(core: &Mutex<App>) -> MutexGuard<'_, App> {
    core.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run headless until `dopus.quit`. `paths` land in the panes before the
/// Bus comes up, so the first `dopus.state` shows them. The Bus is
/// required here: a headless dopus nobody can reach does nothing.
pub fn run(
    config: DOpusConfig,
    config_file: Option<ConfigFile>,
    dirs: Option<AppDirs>,
    service: &str,
    noded_url: &str,
    paths: &[String],
) -> Result<(), String> {
    let meta = ServerMeta {
        service: service.to_owned(),
        headless: true,
        window_available: false,
        config_path: dirs
            .as_ref()
            .map(|d| d.config_dir().join("config.conf.mix").display().to_string()),
        theme_scheme: String::new(),
        theme_mode: String::new(),
        appearance: Default::default(),
        actions: commands::action_table(crate::keymap::default_keys),
        extra_commands: Vec::new(),
    };
    let (mut core, receiver) = DopusCore::new(config, config_file);
    commands::apply_open_paths(&mut core, paths);
    let core = Arc::new(Mutex::new(App::new(core)));
    let (bus, mut deliveries) =
        bus::spawn(service, noded_url, Arc::new(|| {})).map_err(|e| e.to_string())?;
    {
        let core = Arc::clone(&core);
        std::thread::Builder::new()
            .name("dopus-headless-core".to_owned())
            .spawn(move || {
                loop {
                    // Wait outside the lock, so Bus calls are never held up.
                    let event = match receiver.recv_timeout(DRAIN_TICK) {
                        Ok(event) => Some(event),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                    {
                        let mut core = lock(&core);
                        let derived = match event {
                            Some(event) => core.core.on_event(event),
                            None => core.core.tick(Instant::now()),
                        };
                        answer_derived(&mut core, derived, |line| tracing::info!("{line}"));
                    }
                }
            })
            .map_err(|e| format!("core drainer: {e}"))?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("headless runtime: {e}"))?;
    // Only for its command ids and the executed journal: accepted
    // `dopus.execute` runs are published as the window publishes them.
    let commands = crate::actions::registry();
    let topic = toolkit::executed::topic(bus.service());
    tracing::info!("dopus headless as `{service}`");
    runtime.block_on(async {
        while let Some(delivery) = deliveries.next().await {
            if let Delivery::Mail { id, result } = delivery {
                let (rc, body, status) = bus::mail_reply(result);
                tracing::info!("{status}");
                if let Some(id) = id {
                    bus.respond(id, rc, body);
                }
                continue;
            }
            let Delivery::Command(command) = delivery else {
                if delivery == Delivery::Stopped {
                    break;
                }
                continue;
            };
            let mut quit = false;
            let served = if command.command == "dopus.dialog" {
                let args = serde_json::from_str(&command.body).unwrap_or(serde_json::Value::Null);
                let (rc, body) = crate::dialog::command(&mut lock(&core), &args);
                vec![Served::Reply {
                    id: command.id,
                    rc,
                    body: body.to_string(),
                }]
            } else {
                commands::serve_command(
                    &command,
                    &mut lock(&core).core,
                    &meta,
                    &buildinfo::build_info!(),
                )
            };
            let ran = commands::accepted_action(&command, &served);
            for served in served {
                match served {
                    Served::Mail { id, paths } => {
                        if !bus.mail(Some(id), paths) {
                            let (rc, body, _) =
                                bus::mail_reply(Err(bus::MailFailure::unavailable()));
                            bus.respond(id, rc, body);
                        }
                    }
                    Served::Reply { id, rc, body } => bus.respond(id, rc, body),
                    Served::Quit { .. } => quit = true,
                    // serve_command refuses every window effect headless.
                    other => tracing::warn!("headless: unexpected {other:?}"),
                }
            }
            if let Some(id) = ran.filter(|id| commands.get(id).is_some()) {
                commands.note(&id, Origin::Bus);
                let (runs, lost) = commands.take_executed();
                let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
                bus.publish(&topic, bodies, lost);
            }
            if quit {
                break;
            }
        }
    });
    lock(&core).cancel_dialogs();
    let _ = lock(&core).core.flush_config();
    bus.quit();
    // Let the quit's own reply reach the wire before the process goes.
    runtime.block_on(async {
        let wait = async {
            while deliveries
                .next()
                .await
                .is_some_and(|d| d != Delivery::Stopped)
            {}
        };
        let _ = tokio::time::timeout(Duration::from_secs(3), wait).await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pump(
        app: &mut App,
        receiver: &std::sync::mpsc::Receiver<CoreEvent>,
        ready: impl Fn(&App) -> bool,
    ) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let events = match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(event) => app.core.on_event(event),
                Err(_) => app.core.tick(Instant::now()),
            };
            answer_derived(app, events, |_| {});
            if ready(app) {
                return;
            }
            assert!(Instant::now() < until, "headless operation timed out");
        }
    }

    #[test]
    fn typed_answers_create_rename_and_delete_headless() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = DOpusConfig::default();
        config.left.path = dir.path().to_owned();
        config.right.path = dir.path().to_owned();
        let (core, receiver) = DopusCore::new(config, None);
        let mut app = App::new(core);
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"ok"})).1["error_code"],
            "NO_DIALOG"
        );
        app.core.begin_new_folder();
        pump(&mut app, &receiver, |a| a.dialog.is_some());
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"ok","name":"bad/name"})).0,
            10
        );
        assert!(app.dialog.is_some());
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"ok","name":"Fresh"})).0,
            0
        );
        let first = dir.path().join("Fresh");
        pump(&mut app, &receiver, |a| {
            first.is_dir()
                && !a.core.availability().operation_running
                && a.core
                    .pane(dopus_core::PaneId::Left)
                    .root
                    .iter()
                    .any(|e| e.path == first)
        });
        app.core
            .select_path(dopus_core::PaneId::Left, Some(first.clone()));
        app.core.begin_rename();
        pump(&mut app, &receiver, |a| a.dialog.is_some());
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"ok","name":"Renamed"})).0,
            0
        );
        let renamed = dir.path().join("Renamed");
        pump(&mut app, &receiver, |a| {
            renamed.is_dir()
                && !first.exists()
                && !a.core.availability().operation_running
                && a.core
                    .pane(dopus_core::PaneId::Left)
                    .root
                    .iter()
                    .any(|e| e.path == renamed)
        });
        app.core
            .select_path(dopus_core::PaneId::Left, Some(renamed.clone()));
        app.core.delete_selection();
        pump(&mut app, &receiver, |a| a.dialog.is_some());
        assert_eq!(
            crate::dialog::command(&mut app, &json!({})).1["dialog"]["kind"],
            "delete"
        );
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"bad"})).1["error_code"],
            "ARGUMENT"
        );
        assert_eq!(
            crate::dialog::command(&mut app, &json!({"answer":"yes"})).0,
            0
        );
        pump(&mut app, &receiver, |a| {
            !renamed.exists() && !a.core.availability().operation_running
        });
    }

    #[test]
    fn typed_drop_answers_move_copy_or_cancel_the_core_operation() {
        for answer in ["move", "copy", "cancel"] {
            let dir = tempfile::tempdir().unwrap();
            let destination = dir.path().join("destination");
            std::fs::create_dir(&destination).unwrap();
            let source = dir.path().join("source.txt");
            std::fs::write(&source, b"preserved contents").unwrap();
            let mut config = DOpusConfig::default();
            config.left.path = dir.path().to_owned();
            config.right.path = destination.clone();
            let (core, receiver) = DopusCore::new(config, None);
            let mut app = App::new(core);
            app.dropped(dopus_core::PaneId::Left, vec![source.clone()], destination.clone());
            assert_eq!(crate::dialog::command(&mut app, &json!({})).1["dialog"]["kind"], "drop");
            assert_eq!(crate::dialog::command(&mut app, &json!({"answer":answer})), (0, json!({"dialog":null})));
            let target = destination.join("source.txt");
            if answer == "cancel" {
                assert!(!app.core.availability().operation_running);
                assert!(!target.exists());
            } else {
                assert!(app.core.availability().operation_running);
                pump(&mut app, &receiver, |a| target.is_file() && !a.core.availability().operation_running);
                assert_eq!(std::fs::read(&target).unwrap(), b"preserved contents");
            }
            assert_eq!(source.exists(), answer != "move");
            if source.exists() { assert_eq!(std::fs::read(&source).unwrap(), b"preserved contents"); }
        }
    }
}
