// SPDX-License-Identifier: MIT OR Apache-2.0
//! Independent read deadlines. Topic hints wake the reader; they never bypass
//! the expensive gate read's 15-second limit or ask a hidden view to render.
use super::notify;
use crate::program::{Shared, lock};
use citizen::Handle;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const QUEUE: Duration = Duration::from_secs(10);
const HISTORY: Duration = Duration::from_secs(300);
const GATE: Duration = Duration::from_secs(15);
const TOPIC: Duration = Duration::from_secs(2);
const GATE_TIMEOUT: Duration = Duration::from_secs(8);
#[derive(Default)]
struct Schedule {
    queue: Option<Instant>,
    history: Option<Instant>,
    gate: Option<Instant>,
    generation: u64,
}
fn remaining(last: Option<Instant>, interval: Duration, now: Instant) -> Duration {
    last.map_or(Duration::ZERO, |t| {
        interval.saturating_sub(now.saturating_duration_since(t))
    })
}
impl Schedule {
    fn queue_due(&self, now:Instant,hint:bool)->bool {remaining(self.queue,if hint {TOPIC} else {QUEUE},now).is_zero()}
    fn history_due(&self, now: Instant, generation: u64) -> bool {
        (generation != self.generation && remaining(self.history,TOPIC,now).is_zero()) || remaining(self.history, HISTORY, now).is_zero()
    }
    fn gate_due(&self, now: Instant, visible: bool, gating: bool) -> bool {
        visible && gating && remaining(self.gate, GATE, now).is_zero()
    }
    fn wait(&self, now: Instant, visible: bool, gating: bool) -> Duration {
        let mut wait = remaining(self.queue, QUEUE, now).min(remaining(self.history, HISTORY, now));
        if visible && gating {
            wait = wait.min(remaining(self.gate, GATE, now));
        }
        wait.max(Duration::from_millis(100))
    }
}
fn visible(shared: &Mutex<Shared>) -> bool {
    let hosts = {
        let s = lock(shared);
        if s.live.selected != "train" {
            return false;
        }
        s.live.hosts.values().cloned().collect::<Vec<_>>()
    };
    hosts.iter().any(|h| lock(h).is_visible())
}
fn call(handle: &Handle, target: &str, verb: &str, args: Value) -> Result<Value, String> {
    let reply = futures::executor::block_on(handle.raw_within(
        target,
        verb,
        args.to_string(),
        if verb == "train.gate" { GATE_TIMEOUT } else { Duration::from_secs(5) },
    ))
    .map_err(|e| format!("{verb}: {e}"))?;
    if reply.rc != 0 {
        return Err(format!("{verb}: {}", reply.error.unwrap_or(reply.body)));
    }
    let value: Value = serde_json::from_str(&reply.body).map_err(|e| format!("{verb}: {e}"))?;
    if !value.is_object() {
        return Err(format!("{verb}: expected an object"));
    }
    Ok(value)
}
fn read(
    handle: &Handle,
    target: &str,
    verb: &'static str,
    args: Value,
    value: &mut Value,
    errors: &mut BTreeMap<&'static str, String>,
) -> bool {
    match call(handle, target, verb, args) {
        Ok(data) => {
            *value = data;
            errors.remove(verb);
            true
        }
        Err(error) => {
            errors.insert(verb, error);
            false
        }
    }
}
pub fn start_reader(handle: Handle, shared: Arc<Mutex<Shared>>) -> Result<(), String> {
    let signal = lock(&shared).live.refresh.clone();
    std::thread::Builder::new()
        .name("backdrop-train".into())
        .spawn(move || {
            let mut schedule = Schedule::default();
            let mut config = None;
            let mut errors = BTreeMap::new();
            let mut queue_ok = false;
            let mut wake = true;
            loop {
                let (active, train, buildd, hours, generation, mut snapshot) = {
                    let s = lock(&shared);
                    (
                        s.live.selected == "train",
                        s.live.train_service.clone(),
                        s.live.buildd_service.clone(),
                        s.live.hours,
                        s.live.history_generation,
                        (*s.live.snapshot).clone(),
                    )
                };
                let key = (train.clone(), buildd.clone(), hours);
                if config.as_ref() != Some(&key) {
                    schedule.queue = None;
                    schedule.history = None;
                    queue_ok = false;
                    errors.clear();
                    config = Some(key.clone());
                    // Do not reset gate: even rapid configuration changes must not
                    // cause two SSH-backed reads less than 15 seconds apart.
                }
                if active {
                    if schedule.queue_due(Instant::now(),wake) {
                        wake = false;
                        schedule.queue = Some(Instant::now());
                        queue_ok = read(
                            &handle,
                            &train,
                            "train.queue",
                            json!({}),
                            &mut snapshot.queue,
                            &mut errors,
                        );
                        read(
                            &handle,
                            &buildd,
                            "buildd.queue",
                            json!({}),
                            &mut snapshot.workers,
                            &mut errors,
                        );
                    }
                    if schedule.history_due(Instant::now(), generation) {
                        schedule.history = Some(Instant::now());
                        schedule.generation = generation;
                        read(
                            &handle,
                            &train,
                            "train.history",
                            json!({"hours":hours}),
                            &mut snapshot.history,
                            &mut errors,
                        );
                    }
                    let gating = queue_ok && snapshot.gating_batch().is_some();
                    // Recheck visibility immediately before the expensive read.
                    if schedule.gate_due(Instant::now(), visible(&shared), gating) {
                        schedule.gate = Some(Instant::now());
                        read(
                            &handle,
                            &train,
                            "train.gate",
                            json!({}),
                            &mut snapshot.gate,
                            &mut errors,
                        );
                    }
                    if !gating {
                        snapshot.gate = Value::Null;
                        errors.remove("train.gate");
                    }
                    snapshot.hours = hours;
                    snapshot.error = if errors.is_empty() {
                        None
                    } else {
                        Some(errors.values().cloned().collect::<Vec<_>>().join("; "))
                    };
                    {
                        let mut s = lock(&shared);
                        if (
                            s.live.train_service.clone(),
                            s.live.buildd_service.clone(),
                            s.live.hours,
                        ) == key
                            && *s.live.snapshot != snapshot
                        {
                            s.live.snapshot = Arc::new(snapshot);
                        }
                    }
                    notify(&shared, false);
                }
                let gating_now = {
                    let s = lock(&shared);
                    s.live.selected == "train"
                        && queue_ok
                        && s.live.snapshot.gating_batch().is_some()
                };
                let mut wait = if active {
                    schedule.wait(Instant::now(), visible(&shared), gating_now)
                } else {
                    QUEUE
                };
                if active && wake { wait=wait.min(remaining(schedule.queue,TOPIC,Instant::now()).max(Duration::from_millis(100))); }
                if active && generation != schedule.generation {wait=wait.min(remaining(schedule.history,TOPIC,Instant::now()).max(Duration::from_millis(100)));}
                // A pending hint is consumed only after reading: hints delivered
                // during a read survive and trigger the next iteration.
                let pending = lock(&signal.0);
                let (mut pending, _) = signal
                    .1
                    .wait_timeout_while(pending, wait, |p| !*p)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                wake |= *pending;
                *pending = false;
                drop(pending);
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_reads_require_visibility_and_gating_and_keep_their_own_limit() {
        let now = Instant::now();
        let mut s = Schedule::default();
        assert!(!s.gate_due(now, false, true));
        assert!(!s.gate_due(now, true, false));
        assert!(s.gate_due(now, true, true));
        s.gate = Some(now);
        assert!(!s.gate_due(now + Duration::from_secs(14), true, true));
        assert!(s.gate_due(now + GATE, true, true));
    }
    #[test]
    fn history_refreshes_at_five_minutes_or_on_a_train_outcome() {
        let now = Instant::now();
        let s = Schedule {
            history: Some(now),
            generation: 7,
            ..Default::default()
        };
        assert!(!s.history_due(now + Duration::from_secs(299), 7));
        assert!(s.history_due(now + HISTORY, 7));
        assert!(!s.history_due(now, 8));
        assert!(s.history_due(now+TOPIC,8));
    }
    #[test]
    fn gate_deadline_wakes_between_queue_reads_but_hidden_views_ignore_it() {
        let now = Instant::now();
        let s = Schedule {
            queue: Some(now + QUEUE),
            gate: Some(now),
            history: Some(now),
            ..Default::default()
        };
        assert_eq!(s.wait(now + QUEUE, true, true), Duration::from_secs(5));
        assert_eq!(s.wait(now + QUEUE, false, true), QUEUE);
        assert_eq!(s.wait(now + QUEUE, true, false), QUEUE);
    }
    #[test]
    fn topic_storms_have_a_two_second_floor_and_gate_timeout_leaves_other_reads_time() {
        let now=Instant::now();let s=Schedule {queue:Some(now),..Default::default()};
        assert!(!s.queue_due(now,true));assert!(!s.queue_due(now+Duration::from_millis(1999),true));
        assert!(s.queue_due(now+TOPIC,true));assert!(!s.queue_due(now+TOPIC,false));
        assert!(GATE_TIMEOUT<GATE);
    }
}
