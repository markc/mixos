// SPDX-License-Identifier: MIT OR Apache-2.0
//! The same app, verbs and jobs with no window: a Bus service for agents.
//! Nothing asks a person anything here: an action that would discard
//! unsaved work is refused, and there is no clipboard or window to show.
use crate::app::{App, Effect, Pending};
use crate::capture::{Comp, CompBus};
use crate::jobs::{self, Done};
use citizen::Delivery;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;

type Job = BoxFuture<'static, Done>;

/// Serve as `service` on the broker at `url`, capturing through `comp`;
/// `path` is opened first.
pub fn run(service: &str, url: &str, comp: &str, path: Option<PathBuf>) -> Result<(), String> {
    let (handle, mut deliveries) = citizen::start(crate::SERVICE, service, url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let comp_service = comp.to_owned();
    let comp: Arc<dyn Comp> = Arc::new(CompBus {
        handle: handle.clone(),
        service: comp_service.clone(),
    });
    let commands = crate::commands::registry();
    let strings = crate::strings();
    let face = crate::face();
    let mut app = App::new(crate::label, crate::ink::inks(), true);
    app.comp = comp_service;
    jobs::prune();
    if let Some(path) = path {
        app.request(Pending::Open(path));
    }
    runtime.block_on(async {
        let mut jobs: FuturesUnordered<Job> = FuturesUnordered::new();
        let mut cancel: Option<tokio::sync::watch::Sender<bool>> = None;
        loop {
            let mut exit = false;
            for effect in jobs::settle(&mut app, &commands, &strings, false) {
                match effect {
                    Effect::Reply { id, rc, body } => handle.reply(id, rc, body),
                    Effect::Publish { suffix, body } => {
                        let topic = format!("{}.{suffix}", handle.service());
                        handle.publish(&topic, vec![body], 0);
                    }
                    Effect::Capture {
                        ticket, request, ..
                    } => {
                        let (tx, rx) = tokio::sync::watch::channel(false);
                        cancel = Some(tx);
                        jobs.push(Box::pin(jobs::capture(
                            comp.clone(),
                            ticket,
                            request,
                            false,
                            false,
                            rx,
                        )));
                    }
                    Effect::Cancel => {
                        if let Some(tx) = &cancel {
                            let _ = tx.send(true);
                        }
                    }
                    Effect::Open { ticket, path } => jobs.push(Box::pin(jobs::open(ticket, path))),
                    Effect::Save {
                        ticket,
                        document,
                        to,
                    } => jobs.push(Box::pin(jobs::save(ticket, *document, to, face.clone()))),
                    Effect::Copy { ticket, .. } => {
                        app.copied(ticket, Err(crate::label("no-clipboard")));
                    }
                    Effect::Show { ticket } => {
                        if let Some(ticket) = ticket {
                            app.shown(ticket, Err(crate::label("no-window")));
                        }
                    }
                    Effect::Refresh => jobs.push(Box::pin(jobs::refresh(comp.clone()))),
                    Effect::Forget { path } => jobs::forget(&path),
                    Effect::Drive { id, .. } => handle.reply(
                        id,
                        10,
                        serde_json::json!({"error_code":"HEADLESS","message":crate::label("no-window")}),
                    ),
                    Effect::Exit => exit = true,
                    // `settle` answers these.
                    Effect::Commands { .. }
                    | Effect::Execute { .. }
                    | Effect::Describe { .. }
                    | Effect::Recent { .. } => {}
                }
            }
            publish_executed(&handle, &commands);
            // Effects a refusal just queued (a copy, a show) are settled
            // before waiting again.
            if !exit && app.has_effects() {
                continue;
            }
            if exit {
                break;
            }
            tokio::select! {
                done = jobs.next(), if !jobs.is_empty() => {
                    if let Some(done) = done {
                        if matches!(done, Done::Captured(..)) {
                            cancel = None;
                        }
                        jobs::apply(&mut app, done);
                    }
                }
                delivery = deliveries.next() => match delivery {
                    Some(Delivery::Command { id, verb, body }) => app.command(id, &verb, &body),
                    Some(Delivery::Connected) => app.connection(true),
                    Some(Delivery::Disconnected) => app.connection(false),
                    Some(Delivery::Changed | Delivery::Theme | Delivery::Topic(_)) => {}
                    None => break,
                },
            }
        }
    });
    handle.quit();
    handle.wait_done()
}

/// Publish the commands run since the last call.
fn publish_executed(handle: &citizen::Handle, commands: &toolkit::Registry<App>) {
    let (runs, lost) = commands.take_executed();
    if runs.is_empty() && lost == 0 {
        return;
    }
    let topic = toolkit::executed::topic(handle.service());
    let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
    handle.publish(&topic, bodies, lost);
}
