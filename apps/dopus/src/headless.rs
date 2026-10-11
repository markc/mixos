// SPDX-License-Identifier: MIT OR Apache-2.0
//! Headless dopus: the same core and Bus surface, no window. Neighbours can
//! read `dopus.state` (two panes) and drive `dopus.action` against it. The
//! window and this loop share [`verbs::serve_command`]; only the transport
//! differs.
//!
//! A drainer thread owns `on_event` and `tick` (laws 1 and 2) and answers
//! every dialog at once with No (law 3), so nothing waits and no file
//! operation starts. `OpenFile` is refused and logged (law 4). Theme and
//! window actions are refused: there is nothing to paint.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dopus_core::{ConfigFile, ConfirmAnswer, CoreEvent, DOpusConfig, DopusCore};
use futures::StreamExt;

use crate::bus::{self, Delivery};
use crate::dirs::AppDirs;
use crate::verbs::{self, Served, ServerMeta};

/// The drain cadence: how long a worker reply can wait, and the tick period.
const DRAIN_TICK: Duration = Duration::from_millis(200);

/// Answer the core's derived events as a process with no window must:
/// every dialog No, `OpenFile` refused with a log line.
pub fn answer_derived(core: &mut DopusCore, events: Vec<CoreEvent>, log: impl Fn(String)) {
    for event in events {
        match event {
            CoreEvent::ConfirmRequested { token, .. } => core.confirm(token, ConfirmAnswer::No),
            CoreEvent::PromptRequested { token, .. } => core.prompt_text(token, None),
            CoreEvent::OpenFile(path) => log(format!(
                "refusing OpenFile({}), headless never spawns",
                dopus_core::sanitise_display_path(&path)
            )),
            _ => {}
        }
    }
}

fn lock(core: &Mutex<DopusCore>) -> MutexGuard<'_, DopusCore> {
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
    let keymap = crate::keymap::load(dirs.as_ref().map(AppDirs::keymap_file).as_deref())?;
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
        actions: verbs::action_table(&keymap),
        extra_verbs: Vec::new(),
    };
    let (mut core, receiver) = DopusCore::new(config, config_file);
    verbs::apply_open_paths(&mut core, paths);
    let core = Arc::new(Mutex::new(core));
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
                    let derived = {
                        let mut core = lock(&core);
                        match event {
                            Some(event) => core.on_event(event),
                            None => core.tick(Instant::now()),
                        }
                    };
                    answer_derived(&mut lock(&core), derived, |line| tracing::info!("{line}"));
                }
            })
            .map_err(|e| format!("core drainer: {e}"))?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("headless runtime: {e}"))?;
    tracing::info!("dopus headless as `{service}`");
    runtime.block_on(async {
        while let Some(delivery) = deliveries.next().await {
            let Delivery::Command(command) = delivery else {
                if delivery == Delivery::Stopped {
                    break;
                }
                continue;
            };
            let mut quit = false;
            for served in
                verbs::serve_command(&command, &mut lock(&core), &meta, &buildinfo::build_info!())
            {
                match served {
                    Served::Reply { id, rc, body } => bus.respond(id, rc, body),
                    Served::Quit { .. } => quit = true,
                    // serve_command refuses every window effect headless.
                    other => tracing::warn!("headless: unexpected {other:?}"),
                }
            }
            if quit {
                break;
            }
        }
    });
    let _ = lock(&core).flush_config();
    bus.quit();
    // Let the quit's own reply reach the wire before the process goes.
    runtime.block_on(async {
        let wait = async { while deliveries.next().await.is_some_and(|d| d != Delivery::Stopped) {} };
        let _ = tokio::time::timeout(Duration::from_secs(3), wait).await;
    });
    Ok(())
}
