// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Topic events waiting to be published, and the task that publishes them
//! (`<app>.action.executed`, `docs/docs/decisions/2026-10-11-executed-commands.md`).
//!
//! The [`Outbox`] is a bounded queue that drops its oldest event when full,
//! and a running count of events lost. The publisher sends one event at a
//! time, in order, and writes the running count into that event's
//! `dropped` immediately before it sends it; the count then restarts. So
//! every gap a subscriber sees in `seq` is explained by the `dropped` of
//! the next event it receives, whatever was lost on the way.
//!
//! An event counts as lost when it is dropped from a full queue, refused by
//! the broker, or could not be sent at all (the connection was down before
//! the frame was handed over). A send whose outcome is unknown (the
//! connection failed after the frame went out, or the answer never came)
//! may have been published, so it is assumed delivered: a lost
//! acknowledgement must not report a lost event. If it was in fact lost, a
//! subscriber sees a gap that `dropped` does not explain, as with its own
//! missed deliveries.

use ::bus::client::ClientError;
use ::bus::native_client::{SupervisedClient, SupervisedError};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Events waiting for the publisher before the oldest is dropped.
pub const CAPACITY: usize = 256;

/// How long one publish may take.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(2);

/// One queued event and the loss just before it in `seq` order.
#[derive(Debug)]
struct Item {
    topic: String,
    body: Value,
    carried: u64,
}

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<Item>,
    /// Loss before every queued event: sends that failed.
    lost: u64,
    /// Loss after every queued event, for the next event pushed.
    trailing: u64,
}

/// Events waiting to be published on one connection.
#[derive(Debug)]
pub struct Outbox {
    state: Mutex<State>,
    capacity: usize,
    ready: tokio::sync::Notify,
    /// Events the UI could not hand over because the publisher held the
    /// lock: lost, and reported on the first event of the next batch.
    missed: std::sync::atomic::AtomicU64,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new(CAPACITY)
    }
}

/// How many times [`Outbox::push`] retries a busy lock before it counts
/// the batch as lost (it never blocks the UI).
const PUSH_TRIES: usize = 64;

impl Outbox {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            capacity: capacity.max(1),
            ready: tokio::sync::Notify::new(),
            missed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue `bodies` for `topic`, oldest first. `lost` events were lost
    /// just before them, so their loss rides on the first of them. Never
    /// blocks: a full queue drops its oldest event (its loss moves to the
    /// next), and a lock the publisher holds for too long loses the batch.
    pub fn push(&self, topic: &str, bodies: impl IntoIterator<Item = Value>, lost: u64) {
        let bodies: Vec<Value> = bodies.into_iter().collect();
        let mut tries = 0;
        let mut state = loop {
            match self.state.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) if tries < PUSH_TRIES => {
                    tries += 1;
                    std::thread::yield_now();
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    let n = bodies.len() as u64 + lost;
                    self.missed
                        .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
        };
        // Batches the UI could not hand over came before this one.
        let missed = self.missed.swap(0, std::sync::atomic::Ordering::Relaxed);
        let mut carried = std::mem::take(&mut state.trailing) + missed + lost;
        if bodies.is_empty() {
            state.trailing = carried;
            return;
        }
        for body in bodies {
            if state.queue.len() >= self.capacity
                && let Some(evicted) = state.queue.pop_front()
            {
                let moved = evicted.carried + 1;
                match state.queue.front_mut() {
                    Some(next) => next.carried += moved,
                    None => carried += moved,
                }
            }
            state.queue.push_back(Item {
                topic: topic.to_owned(),
                body,
                carried: std::mem::take(&mut carried),
            });
        }
        drop(state);
        self.ready.notify_one();
    }

    /// The next event to send, with every loss before it written into its
    /// `dropped`.
    pub fn next(&self) -> Option<(String, Value)> {
        let mut state = self.state();
        let item = state.queue.pop_front()?;
        let lost = std::mem::take(&mut state.lost) + item.carried;
        let mut body = item.body;
        if let Some(fields) = body.as_object_mut() {
            let key = if fields.get("dropped").is_some_and(Value::is_boolean) {
                "event_dropped"
            } else {
                "dropped"
            };
            fields.insert(key.into(), Value::from(lost));
        }
        Some((item.topic, body))
    }

    /// The event just taken was not published: it is lost, and so is the
    /// loss it carried.
    pub fn failed(&self, body: &Value) {
        let carried = body
            .get("event_dropped")
            .or_else(|| body.get("dropped"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.state().lost += 1 + carried;
    }

    /// How many events wait (tests).
    pub fn len(&self) -> usize {
        self.state().queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The Bus frame a subscriber receives: the topic as the inner command and
/// the event as its body (the shape Mix's `publish` sends).
pub fn frame(topic: &str, body: &Value) -> String {
    // The wire encoder ends the message with a newline, as for Mix's
    // `publish`; the payload itself carries none.
    format!("---\ncommand: {topic}\n---\n{body}")
}

/// Whether a failed publish certainly did not reach subscribers.
fn certainly_unsent(error: &SupervisedError) -> bool {
    match error {
        SupervisedError::Disconnected
        | SupervisedError::ShuttingDown
        | SupervisedError::InitialConnectFailed { .. } => true,
        SupervisedError::Transport(e) => matches!(
            e,
            ClientError::Connect(_)
                | ClientError::Json(_)
                | ClientError::Refused { .. }
                | ClientError::Rejected(_)
        ),
    }
}

/// Publish `outbox`'s events on `client`, one at a time, until the task is
/// dropped (with the connection's runtime).
pub fn spawn_publisher(client: Arc<SupervisedClient>, outbox: Arc<Outbox>) {
    tokio::spawn(async move {
        loop {
            let ready = outbox.ready.notified();
            let Some((topic, body)) = outbox.next() else {
                ready.await;
                continue;
            };
            let headers = BTreeMap::from([
                ("name".to_owned(), topic.clone()),
                ("retain".to_owned(), "false".to_owned()),
            ]);
            let sent = client
                .call_with_headers_raw_within(
                    "noded",
                    "topic.publish",
                    &headers,
                    &frame(&topic, &body),
                    PUBLISH_TIMEOUT,
                )
                .await;
            let lost = match &sent {
                Ok((rc, _, _)) => *rc != 0,
                Err(error) => certainly_unsent(error),
            };
            if lost {
                outbox.failed(&body);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(seq: u64) -> Value {
        json!({"v": 2, "seq": seq, "id": "x", "params": {"n": 4}, "result": [1, 2], "dropped": 0})
    }

    #[test]
    fn the_loss_rides_on_the_next_event_sent() {
        let outbox = Outbox::new(2);
        outbox.push("t", [event(2), event(3), event(4)], 1); // 1 lost before, 2 pushed out
        let (_, first) = outbox.next().unwrap();
        assert_eq!(first["v"], 2);
        assert_eq!(first["params"], json!({"n": 4}));
        assert_eq!(first["result"], json!([1, 2]));
        assert_eq!(
            (first["seq"].as_u64(), first["dropped"].as_u64()),
            (Some(3), Some(2))
        );
        outbox.failed(&first); // 3 lost now: itself and the 2 it carried
        let (_, second) = outbox.next().unwrap();
        assert_eq!(
            (second["seq"].as_u64(), second["dropped"].as_u64()),
            (Some(4), Some(3))
        );
        assert!(outbox.next().is_none());
        assert!(outbox.is_empty());
    }

    #[test]
    fn a_batch_loss_rides_on_that_batch_not_on_an_older_event() {
        let outbox = Outbox::new(8);
        outbox.push("t", [event(1)], 0);
        outbox.push("t", [event(3)], 1); // seq 2 was lost just before seq 3
        let (_, first) = outbox.next().unwrap();
        let (_, second) = outbox.next().unwrap();
        assert_eq!(
            (first["seq"].as_u64(), first["dropped"].as_u64()),
            (Some(1), Some(0))
        );
        assert_eq!(
            (second["seq"].as_u64(), second["dropped"].as_u64()),
            (Some(3), Some(1))
        );
        // A loss with nothing after it yet waits for the next batch.
        outbox.push("t", Vec::new(), 2);
        outbox.push("t", [event(6)], 0);
        let (_, third) = outbox.next().unwrap();
        assert_eq!(third["dropped"].as_u64(), Some(2));
    }

    #[test]
    fn the_frame_is_what_subscribers_receive() {
        let text = frame("demo.action.executed", &event(1));
        assert!(text.starts_with("---\ncommand: demo.action.executed\n---\n{"));
        let body: Value = serde_json::from_str(text.lines().nth(3).unwrap()).unwrap();
        assert_eq!(body["seq"], 1);
    }

    #[test]
    fn macro_output_loss_is_independent_of_event_loss() {
        let outbox = Outbox::new(2);
        outbox.push("demo.macros.finished", [json!({"run":1,"dropped":true})], 3);
        let (_, body) = outbox.next().unwrap();
        assert_eq!(body["dropped"], true);
        assert_eq!(body["event_dropped"], 3);
        outbox.failed(&body);
        outbox.push(
            "demo.macros.finished",
            [json!({"run":2,"dropped":false})],
            0,
        );
        let (_, body) = outbox.next().unwrap();
        assert_eq!(body["dropped"], false);
        assert_eq!(body["event_dropped"], 4);
    }
}
