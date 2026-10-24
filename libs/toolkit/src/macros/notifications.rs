// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain result toasts through one lazy native Bus client per process.
use super::Finished;
use ::bus::native_client::SupervisedClient;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::time::Duration;

const SEND_BUDGET: Duration = Duration::from_secs(2);

struct Message {
    app: String,
    payload: serde_json::Value,
    finished: Finished,
    tx: mpsc::Sender<Finished>,
    alive: Weak<()>,
    ctx: egui::Context,
    viewport: egui::ViewportId,
}

impl Message {
    fn fallback(self) {
        if self.alive.upgrade().is_some() && self.tx.send(self.finished).is_ok() {
            self.ctx.request_repaint_of(self.viewport);
        }
    }
}

static CLIENT: OnceLock<Option<tokio::sync::mpsc::Sender<Message>>> = OnceLock::new();

pub(super) fn post(
    app: &str,
    payload: serde_json::Value,
    finished: Finished,
    tx: mpsc::Sender<Finished>,
    alive: &Arc<()>,
    ctx: &egui::Context,
) {
    let sender = CLIENT.get_or_init(|| {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(64);
        std::thread::Builder::new()
            .name("macro-notifications".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    rx.close();
                    while let Ok(message) = rx.try_recv() {
                        message.fallback();
                    }
                    return;
                };
                runtime.block_on(async move {
                    let mut client = None;
                    while let Some(message) = rx.recv().await {
                        if message.alive.upgrade().is_none() {
                            continue;
                        }
                        let sent = tokio::time::timeout(SEND_BUDGET, async {
                            if client.is_none() {
                                let service =
                                    format!("{}-macros-{}", message.app, std::process::id());
                                client = SupervisedClient::connect_options(
                                    &service,
                                    &::bus::client_helpers::resolve_noded_url(),
                                )
                                .fatal_on_registration_rejection(true)
                                .connect()
                                .await
                                .ok();
                            }
                            let (rc, _, _) = client
                                .as_ref()?
                                .call_with_headers_raw(
                                    "notify",
                                    "notify.send",
                                    &BTreeMap::new(),
                                    &message.payload.to_string(),
                                )
                                .await
                                .ok()?;
                            (rc < 10).then_some(())
                        })
                        .await
                        .ok()
                        .flatten()
                        .is_some();
                        if !sent {
                            message.fallback();
                        }
                    }
                });
                runtime.shutdown_timeout(Duration::from_millis(100));
            })
            .ok()
            .map(|_| tx)
    });
    let message = Message {
        app: app.into(),
        payload,
        finished,
        tx,
        alive: Arc::downgrade(alive),
        ctx: ctx.clone(),
        viewport: ctx.viewport_id(),
    };
    match sender {
        Some(sender) => {
            if let Err(error) = sender.try_send(message) {
                error.into_inner().fallback();
            }
        }
        None => message.fallback(),
    }
}
