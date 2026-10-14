// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of the `term` Bus name.
//!
//! term-core's lane owns the connection and serves the terminal's own
//! verbs (`term.tabs`, `term.pane.split`, …) on its thread. The verbs only
//! the window can answer reach it as [`WindowEvent`]s: the toolkit's drive
//! verbs (`term.ui.*`, `term.window`, `term.window.state`), the command
//! registry (`term.commands`, `term.execute`) and `app.describe`. Topic
//! frames the window subscribed come the same way.
//!
//! Once the name is registered the lane hands over the client and its
//! runtime ([`term_core::bus::Link`]); the window's own calls (the session
//! look from settingsd), subscriptions and timers run there, and each
//! answer comes back as a [`Delivery`] with a repaint request. Nothing
//! polls.

use ::bus::native_client::ConnState;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use term_core::bus::{Link, WindowEvent};
use toolkit::drive;

/// The Bus name and verb namespace this window serves.
pub const APP: &str = "term";

/// How long one session-look read may take.
const LOOK_BUDGET: Duration = Duration::from_secs(30);

/// Whether the window answers `verb` (term-core's [`term_core::bus::Window`]
/// routing test).
pub fn answers(verb: &str) -> bool {
    matches!(verb, "app.describe" | "term.commands" | "term.execute")
        || verb
            .strip_prefix("term.")
            .is_some_and(|v| drive::VERBS.contains(&v))
}

/// The HELP line for `verb`, after the verb and its argument shape.
fn help_line(verb: &str) -> Option<&'static str> {
    term_core::bus::HELP
        .lines()
        .find(|line| line.split([' ', ':']).next() == Some(verb))
}

/// The whole Bus surface: the terminal's verbs, the window's command verbs
/// and the drive verbs.
pub fn describe() -> Value {
    let mut verbs = vec![
        json!({"name":"HELP","description":"List every verb","read_only":true}),
        json!({"name":"app.describe","description":"Describe the app and every verb with its arguments","read_only":true}),
    ];
    for (name, read_only) in term_core::bus::VERBS {
        let description = help_line(name).unwrap_or(match name {
            "term.session" => {
                "term.session {}: this process's pid, instance and native session status"
            }
            _ => name,
        });
        verbs.push(json!({"name":name,"description":description,"read_only":read_only}));
    }
    verbs.push(json!({"name":"term.commands","description":"List the window's commands with labels, menus, shortcuts and enablement","read_only":true}));
    verbs.push(json!({"name":"term.execute","args":{"id":"string","tab":"optional tab id: the tab tab.select or tab.close acts on (default the active one)","pane":"optional pane id: the pane pane.focus focuses"},"description":"Run one window command by id, as its menu item or shortcut would (tab.new, pane.split_right, view.zoom_in, edit.paste, app.quit, …; term.commands lists them)","read_only":false}));
    verbs.extend(drive::describe(APP));
    json!({"schema":"term.v1","app_id":crate::APP_ID,"verbs":verbs})
}

/// What the window's own Bus work hands back.
#[derive(Debug)]
pub enum Delivery {
    /// A subscription landed: frames on the topic arrive from now on.
    Subscribed(String),
    /// The answer to session-look read `ticket`: rc, body and the broker's
    /// error header, or `None` when the call failed or timed out.
    Look {
        ticket: u64,
        reply: Option<(u8, String, Option<String>)>,
    },
    /// A look retry is due.
    LookRetry { generation: u64 },
    /// The answer to a settings write ([`WindowBus::write_settings`]), as
    /// [`Delivery::Look`]'s.
    Written {
        reply: Option<(u8, String, Option<String>)>,
    },
    /// A font size write is due (the zoom keys have paused).
    FontDue { generation: u64 },
    /// The connection came (back) up.
    Connected,
}

/// The window's handle on the Bus.
pub struct WindowBus {
    link: Arc<OnceLock<Link>>,
    events: Receiver<WindowEvent>,
    tx: Sender<Delivery>,
    rx: Receiver<Delivery>,
    outbox: Arc<citizen::outbox::Outbox>,
    wake: Arc<dyn Fn() + Send + Sync>,
    started: bool,
}

impl WindowBus {
    pub fn new(
        link: Arc<OnceLock<Link>>,
        events: Receiver<WindowEvent>,
        outbox: Arc<citizen::outbox::Outbox>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (tx, rx) = channel();
        Self {
            link,
            events,
            tx,
            rx,
            outbox,
            wake,
            started: false,
        }
    }

    /// A window with no Bus (tests, or no broker).
    pub fn offline() -> Self {
        let (_, events) = channel();
        Self::new(
            Arc::new(OnceLock::new()),
            events,
            Arc::new(citizen::outbox::Outbox::default()),
            Arc::new(|| {}),
        )
    }

    fn link(&self) -> Option<&Link> {
        self.link.get()
    }

    pub fn connected(&self) -> bool {
        self.link().is_some_and(|l| l.client.is_connected())
    }

    /// The name registered, once it is.
    pub fn name(&self) -> &str {
        self.link().map_or(APP, |l| l.name.as_str())
    }

    /// Once the name is registered: subscribe `topics` and follow the
    /// connection. True the one time it starts.
    pub fn start(&mut self, topics: &[String]) -> bool {
        if self.started || self.link().is_none() {
            return false;
        }
        self.started = true;
        for topic in topics {
            self.subscribe(topic.clone());
        }
        let Some(link) = self.link() else {
            return false;
        };
        let (client, tx, wake) = (link.client.clone(), self.tx.clone(), self.wake.clone());
        link.runtime.spawn(async move {
            let mut state = client.subscribe_state();
            while state.changed().await.is_ok() {
                let (connected, ended) = match *state.borrow_and_update() {
                    ConnState::Connected => (true, false),
                    ConnState::ShuttingDown | ConnState::Fatal => (false, true),
                    _ => (false, false),
                };
                if ended {
                    break;
                }
                if connected {
                    let _ = tx.send(Delivery::Connected);
                    wake();
                }
            }
        });
        true
    }

    /// Subscribe `topic`, retrying until it lands; [`Delivery::Subscribed`]
    /// follows.
    pub fn subscribe(&self, topic: String) {
        let Some(link) = self.link() else {
            return;
        };
        let (client, tx, wake) = (link.client.clone(), self.tx.clone(), self.wake.clone());
        link.runtime.spawn(async move {
            // The client replays only topics that once subscribed.
            let mut delay = Duration::from_millis(250);
            loop {
                if matches!(client.state(), ConnState::ShuttingDown | ConnState::Fatal) {
                    return;
                }
                match client.subscribe_topic(&topic).await {
                    Ok(()) => break,
                    Err(_) => {
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(5));
                    }
                }
            }
            let _ = tx.send(Delivery::Subscribed(topic));
            wake();
        });
    }

    /// Perform a session-look read; [`Delivery::Look`] answers it. False
    /// with no Bus, so the shell can settle the failed send immediately.
    pub fn read_look(&self, read: settings::follow::Read) -> bool {
        let Some(link) = self.link() else {
            return false;
        };
        let (client, tx, wake) = (link.client.clone(), self.tx.clone(), self.wake.clone());
        link.runtime.spawn(async move {
            let (body, headers) = (read.body.to_string(), BTreeMap::new());
            let call = client.call_with_headers_raw(read.service, read.verb, &headers, &body);
            let reply = match tokio::time::timeout(LOOK_BUDGET, call).await {
                Ok(Ok(answer)) => Some(answer),
                _ => None,
            };
            let _ = tx.send(Delivery::Look {
                ticket: read.ticket,
                reply,
            });
            wake();
        });
        true
    }

    /// Deliver [`Delivery::LookRetry`] with `generation` after `delay`.
    pub fn retry_look(&self, delay: Duration, generation: u64) {
        self.after(delay, Delivery::LookRetry { generation });
    }

    /// Deliver [`Delivery::FontDue`] with `generation` after `delay`.
    pub fn font_due(&self, delay: Duration, generation: u64) {
        self.after(delay, Delivery::FontDue { generation });
    }

    fn after(&self, delay: Duration, delivery: Delivery) {
        let Some(link) = self.link() else {
            return;
        };
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        link.runtime.spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(delivery);
            wake();
        });
    }

    /// Perform a settings write (a [`settings::follow::Follower::apply`]);
    /// [`Delivery::Written`] answers it. False with no Bus.
    pub fn write_settings(&self, write: settings::follow::Read) -> bool {
        let Some(link) = self.link() else {
            return false;
        };
        let (client, tx, wake) = (link.client.clone(), self.tx.clone(), self.wake.clone());
        link.runtime.spawn(async move {
            let (body, headers) = (write.body.to_string(), BTreeMap::new());
            let call = client.call_with_headers_raw(write.service, write.verb, &headers, &body);
            let reply = match tokio::time::timeout(LOOK_BUDGET, call).await {
                Ok(Ok(answer)) => Some(answer),
                _ => None,
            };
            let _ = tx.send(Delivery::Written { reply });
            wake();
        });
        true
    }

    /// Queue `bodies` for publishing on `<name>.<suffix>`.
    pub fn publish(&self, topic: &str, bodies: Vec<Value>, lost: u64) {
        self.outbox.push(topic, bodies, lost);
    }

    /// The next verb or topic frame from the lane.
    pub fn next_event(&self) -> Option<WindowEvent> {
        self.events.try_recv().ok()
    }

    /// The next answer to the window's own Bus work.
    pub fn next_delivery(&self) -> Option<Delivery> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_answers_exactly_its_own_verbs() {
        for verb in drive::VERBS {
            assert!(answers(&format!("term.{verb}")), "{verb}");
        }
        for verb in ["app.describe", "term.commands", "term.execute"] {
            assert!(answers(verb));
        }
        for (verb, _) in term_core::bus::VERBS {
            assert!(!answers(verb), "{verb} is the lane's");
        }
        assert!(!answers("HELP") && !answers("ced.ui.tree"));
    }

    #[test]
    fn every_described_verb_has_a_description() {
        let surface = describe();
        for verb in surface["verbs"].as_array().unwrap() {
            let name = verb["name"].as_str().unwrap();
            let description = verb["description"].as_str().unwrap();
            assert!(description.len() > name.len(), "{name}: {description}");
        }
    }
}
