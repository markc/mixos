// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ced --headless`: the controller and the Bus thread with no window. Every
//! `ced.*` verb works except `ced.layout` (UNAVAILABLE), so agents and tests
//! can drive the real model.
//!
//! The loop blocks on the Bus thread's delivery channel and wakes only for a
//! delivery: a reply, topic, timer, deadline, connection edge or `ced.*`
//! command. Clipboard reads answer "empty" (there is no clipboard), notices
//! and prompts go to stderr (recovered documents stay in the edit service
//! for a window to open), Mix lexing runs inline (there is no frame to
//! keep), and the session is written when the controller asks (debounced
//! there) and on quit.
//!
//! Quitting finishes the effect batch it came in, so its answer is sent,
//! then stops the Bus and waits for it to drain.

use documents::config::Config;
use documents::controller::{Controller, Effect};
use documents::dirs::{AppDirs, COMPONENT};
use documents::session;
use editor_model::types::Intent;
use futures::StreamExt;

use crate::bus::{self, Delivery};

/// Run until `app.quit` (or File › Exit over the Bus). `service` is the Bus
/// name, `url` the broker; `paths` are opened at start.
pub fn run(service: &str, url: &str, config: Config, paths: &[String]) -> Result<(), String> {
    let (bus, mut deliveries) = bus::spawn(service, url, std::sync::Arc::new(|| {}))
        .map_err(|e| format!("ced --headless: {e}"))?;
    let dirs = AppDirs::resolve(COMPONENT);
    let session_path = dirs.as_ref().map(AppDirs::session_file);
    let mut ctl = Controller::new(config, crate::run_id(), true);
    ctl.set_paths(
        dirs.as_ref()
            .map(|d| d.config_file().to_string_lossy().into_owned()),
        session_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    if let Some(p) = &session_path {
        ctl.set_session(session::load(p));
    }
    let mut work = ctl.start();
    if !paths.is_empty() {
        work.extend(ctl.open_paths(paths, Intent::ui(0)));
    }
    let mut quitting = false;
    let mut stopping = false;
    futures::executor::block_on(async {
        loop {
            // Perform everything pending; the controller may answer an
            // effect (a clipboard read, a relex) with more.
            while !work.is_empty() {
                let mut next = Vec::new();
                for effect in std::mem::take(&mut work) {
                    match effect {
                        Effect::SaveSession => save(&ctl, session_path.as_deref()),
                        Effect::Quit => quitting = true,
                        Effect::Notice { tab, notice } => {
                            eprintln!("ced: tab {tab:?}: {notice:?}");
                        }
                        Effect::Prompt(prompt) => eprintln!("ced: needs a window: {prompt:?}"),
                        Effect::ClipboardRead { intent, .. } => {
                            next.extend(ctl.on_paste(intent, None));
                        }
                        Effect::Relex { tab, tag, source } => {
                            let spans = editor_model::highlight::run_mix(&tag.language, &source);
                            ctl.on_relex(tab, tag, spans);
                        }
                        other => bus.perform(&other),
                    }
                }
                work = next;
            }
            if quitting {
                // The answers queued above go out before the Bus closes.
                save(&ctl, session_path.as_deref());
                bus.shutdown(None);
                quitting = false;
                stopping = true;
                continue;
            }
            let Some(delivery) = deliveries.next().await else {
                save(&ctl, session_path.as_deref());
                return Err("the Bus thread ended".to_owned());
            };
            work = match delivery {
                Delivery::Incoming(i) => ctl.on_incoming(i),
                Delivery::Command(c) => ctl.on_bus_command(c),
                Delivery::Stopped { faults } => {
                    for fault in &faults {
                        eprintln!("ced: {fault}");
                    }
                    // Stopped without being asked: the broker refused or
                    // ended the connection.
                    return if stopping {
                        Ok(())
                    } else {
                        Err("the Bus connection stopped".to_owned())
                    };
                }
            };
        }
    })
}

fn save(ctl: &Controller, path: Option<&std::path::Path>) {
    if let Some(p) = path
        && let Err(e) = session::save(p, &ctl.session())
    {
        eprintln!("ced: session not saved to {}: {e}", p.display());
    }
}
