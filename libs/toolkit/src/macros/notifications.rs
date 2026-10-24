// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plain result toasts through one lazy native Bus client per process.
use super::Finished;
use ::bus::native_client::{ConnState, SupervisedClient};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::time::Duration;

const SEND_BUDGET: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_millis(250);

trait ToastClient {
    fn terminal(&self) -> bool;
    async fn send(&self, payload: &serde_json::Value) -> bool;
}

impl ToastClient for SupervisedClient {
    fn terminal(&self) -> bool {
        matches!(self.state(), ConnState::Fatal | ConnState::ShuttingDown)
    }

    async fn send(&self, payload: &serde_json::Value) -> bool {
        self.call_with_headers_raw(
            "notify",
            "notify.send",
            &BTreeMap::new(),
            &payload.to_string(),
        )
        .await
        .is_ok_and(|(rc, _, _)| rc < 10)
    }
}

struct Delivery<C> {
    client: Option<C>,
    retry_after: tokio::time::Instant,
}

impl<C: ToastClient> Delivery<C> {
    fn new() -> Self {
        Self {
            client: None,
            retry_after: tokio::time::Instant::now(),
        }
    }

    async fn post<F, Fut>(&mut self, payload: &serde_json::Value, connect: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<C>>,
    {
        if self.client.as_ref().is_some_and(ToastClient::terminal) {
            self.client = None;
            self.retry_after = tokio::time::Instant::now() + RETRY_DELAY;
        }
        if tokio::time::Instant::now() < self.retry_after {
            return false;
        }
        let sent = tokio::time::timeout(SEND_BUDGET, async {
            if self.client.is_none() {
                self.client = connect().await;
            }
            match &self.client {
                Some(client) => client.send(payload).await,
                None => false,
            }
        })
        .await
        .unwrap_or(false);
        if !sent {
            self.client = None;
            self.retry_after = tokio::time::Instant::now() + RETRY_DELAY;
        }
        sent
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        terminal: bool,
        succeeds: bool,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl ToastClient for Fake {
        fn terminal(&self) -> bool {
            self.terminal
        }
        async fn send(&self, _: &serde_json::Value) -> bool {
            self.succeeds
        }
    }

    #[tokio::test]
    async fn failed_and_terminal_clients_are_dropped_then_replaced() {
        for terminal in [false, true] {
            let drops = Arc::new(AtomicUsize::new(0));
            let mut delivery = Delivery::new();
            delivery.client = Some(Fake {
                terminal,
                succeeds: false,
                drops: drops.clone(),
            });
            let payload = serde_json::json!({});
            assert!(
                !delivery
                    .post(&payload, || async {
                        panic!("must not connect before backoff")
                    })
                    .await
            );
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert!(delivery.client.is_none());
            assert!(
                !delivery
                    .post(&payload, || async { panic!("backoff") })
                    .await
            );
            delivery.retry_after = tokio::time::Instant::now();
            assert!(
                delivery
                    .post(&payload, || async {
                        Some(Fake {
                            terminal: false,
                            succeeds: true,
                            drops: drops.clone(),
                        })
                    })
                    .await
            );
            assert!(delivery.client.is_some());
        }
    }
}

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
                    let mut delivery = Delivery::new();
                    while let Some(message) = rx.recv().await {
                        if message.alive.upgrade().is_none() {
                            continue;
                        }
                        let sent = delivery
                            .post(&message.payload, || async {
                                let service =
                                    format!("{}-macros-{}", message.app, std::process::id());
                                SupervisedClient::connect_options(
                                    &service,
                                    &::bus::client_helpers::resolve_noded_url(),
                                )
                                .fatal_on_registration_rejection(true)
                                .connect()
                                .await
                                .ok()
                            })
                            .await;
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
