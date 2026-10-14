// SPDX-License-Identifier: MIT OR Apache-2.0
//! Zoom persistence decisions. The shell supplies time and performs actions.

use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(400);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WriteReply {
    Saved,
    Conflict,
    Refused,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event {
    Zoom(f32),
    Reset(f32),
    WriteSettled(WriteReply),
    ReadRequested,
    ReadAnswered {
        usable: bool,
        following: bool,
        /// The follower retargeted and has already prepared another read.
        continued: bool,
        retry: Option<Duration>,
    },
    TimerFired(u64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Write(f32),
    Read,
    Schedule {
        delay: Duration,
        generation: u64,
    },
    CancelTimer,
    /// Ordinary appearance following, separate from zoom persistence.
    RetryLook(Duration),
}

#[derive(Debug, Default)]
pub(crate) struct Persistence {
    pub(crate) pending: Option<f32>,
    sent: Option<f32>,
    following: bool,
    reading: bool,
    read_again: bool,
    /// Only a read started after the conflict can supply its fresh fence.
    fresh_read: bool,
    persistence_read: bool,
    refresh: bool,
    attempts: u32,
    retry_at: Option<Instant>,
    generation: u64,
}

impl Persistence {
    fn cancel_timer(&mut self, actions: &mut Vec<Action>) {
        if self.retry_at.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            actions.push(Action::CancelTimer);
        }
    }

    fn schedule(&mut self, delay: Duration, now: Instant, actions: &mut Vec<Action>) {
        self.cancel_timer(actions);
        self.generation = self.generation.wrapping_add(1);
        self.retry_at = Some(now + delay);
        actions.push(Action::Schedule {
            delay,
            generation: self.generation,
        });
    }

    fn back_off(&mut self, now: Instant, actions: &mut Vec<Action>) {
        self.attempts = (self.attempts + 1).min(6);
        let delay = Duration::from_secs(1 << (self.attempts - 1)).min(MAX_BACKOFF);
        self.schedule(delay, now, actions);
    }

    fn read(&mut self, actions: &mut Vec<Action>) {
        if self.reading {
            self.read_again = true;
        } else {
            self.reading = true;
            self.fresh_read = self.refresh;
            self.persistence_read = self.pending.is_some();
            actions.push(Action::Read);
        }
    }

    fn advance(&mut self, actions: &mut Vec<Action>) {
        if self.pending.is_none() {
            self.cancel_timer(actions);
            self.refresh = false;
            return;
        }
        // One live path owns the pending zoom. A due but undelivered timer
        // remains live until TimerFired consumes it; wall time never cancels it.
        if self.sent.is_some() || self.reading || self.retry_at.is_some() {
            return;
        }
        if self.refresh || !self.following {
            self.read(actions);
        } else {
            self.sent = self.pending;
            actions.push(Action::Write(self.sent.expect("pending zoom")));
        }
    }

    pub(crate) fn event(&mut self, event: Event, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        match event {
            Event::Zoom(size) => {
                self.pending = Some(size);
                self.attempts = 0;
                self.cancel_timer(&mut actions);
                if self.sent.is_none() && !self.reading {
                    self.schedule(QUIET, now, &mut actions);
                }
            }
            Event::Reset(profile_size) => {
                self.pending = self
                    .sent
                    .filter(|size| *size != profile_size)
                    .map(|_| profile_size);
                self.attempts = 0;
                self.cancel_timer(&mut actions);
                if self.pending.is_none() {
                    if self.refresh {
                        self.following = false;
                    }
                    self.refresh = false;
                    // Discard a conflict's coalesced retry, not the active read.
                    self.read_again = false;
                }
            }
            Event::WriteSettled(reply) => {
                let Some(sent) = self.sent.take() else {
                    return actions;
                };
                match reply {
                    WriteReply::Saved => {
                        self.attempts = 0;
                        self.cancel_timer(&mut actions);
                        if self.pending == Some(sent) {
                            self.pending = None;
                        }
                        // Read the committed revision before correcting an
                        // in-flight A with B (or the profile size after reset).
                        self.refresh = self.pending.is_some();
                        self.fresh_read = false;
                        self.read(&mut actions);
                    }
                    WriteReply::Conflict => {
                        self.cancel_timer(&mut actions);
                        if self.pending.is_some() {
                            self.refresh = true;
                            self.fresh_read = false;
                            self.read(&mut actions);
                        }
                    }
                    WriteReply::Refused => {
                        if self.pending == Some(sent) {
                            self.pending = None;
                        }
                        self.attempts = 0;
                        self.cancel_timer(&mut actions);
                        // A newer zoom was not refused and may still be saved.
                        // Also restore a refused zoom from the profile's look.
                        self.read(&mut actions);
                    }
                    WriteReply::Failed => {
                        if self.pending == Some(sent) {
                            self.back_off(now, &mut actions);
                        }
                    }
                }
            }
            Event::ReadRequested => {
                // Hints cannot bypass a failed refresh's backoff. Once reset
                // discards the zoom, ordinary following is free to read again.
                if !(self.refresh && self.retry_at.is_some()) {
                    self.read(&mut actions);
                }
            }
            Event::ReadAnswered {
                usable,
                following,
                continued,
                retry,
            } => {
                self.following = following;
                if continued {
                    // Retargeting stays on the same fresh-read path.
                    actions.push(Action::Read);
                    return actions;
                }
                self.reading = false;
                let again = std::mem::take(&mut self.read_again);
                if usable {
                    self.attempts = 0;
                    if self.refresh && self.fresh_read && !again {
                        self.refresh = false;
                        self.cancel_timer(&mut actions);
                    }
                    if again || (self.refresh && !self.fresh_read) {
                        self.read(&mut actions);
                    }
                } else if self.pending.is_some() {
                    // A cached fence is no substitute for the failed refresh.
                    self.refresh = true;
                    if self.sent.is_none() {
                        self.back_off(now, &mut actions);
                    }
                } else if again {
                    self.read(&mut actions);
                } else if !self.persistence_read {
                    if let Some(delay) = retry {
                        actions.push(Action::RetryLook(delay));
                    }
                }
            }
            Event::TimerFired(generation) => {
                if generation != self.generation || self.retry_at.is_none() {
                    return actions;
                }
                let at = self.retry_at.expect("scheduled timer");
                if now < at {
                    // An early delivery still has a strictly positive delay.
                    self.schedule(at - now, now, &mut actions);
                } else {
                    self.retry_at = None;
                }
            }
        }
        self.advance(&mut actions);
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: Event = Event::ReadAnswered {
        usable: true,
        following: true,
        continued: false,
        retry: None,
    };
    const BAD: Event = Event::ReadAnswered {
        usable: false,
        // A lost call/overload leaves the follower's cached fence present.
        following: true,
        continued: false,
        retry: Some(Duration::from_millis(250)),
    };

    #[derive(Debug, PartialEq)]
    enum Effect {
        Write(f32),
        Read,
        Timer(u64),
        Cancel,
        LookRetry(u64),
    }

    enum Step {
        Input(Event, Vec<Effect>),
        Wait(u64),
        Fire(Vec<Effect>),
        Stale,
        Pending(Option<f32>),
    }

    fn ready() -> Vec<Step> {
        vec![
            Step::Input(Event::ReadRequested, vec![Effect::Read]),
            Step::Input(GOOD, vec![]),
        ]
    }

    fn writing() -> Vec<Step> {
        let mut steps = ready();
        steps.extend([
            Step::Input(Event::Zoom(16.0), vec![Effect::Timer(400)]),
            Step::Wait(400),
            Step::Fire(vec![Effect::Write(16.0)]),
        ]);
        steps
    }

    fn failed_refresh() -> Vec<Step> {
        let mut steps = writing();
        steps.extend([
            Step::Input(
                Event::WriteSettled(WriteReply::Conflict),
                vec![Effect::Read],
            ),
            Step::Input(BAD, vec![Effect::Timer(1000)]),
        ]);
        steps
    }

    #[test]
    fn persistence_sequences_preserve_live_paths_and_retry_invariants() {
        use Effect::{Cancel, Read, Timer, Write};
        use Step::{Fire, Input, Pending, Stale, Wait};
        let mut cases = Vec::new();

        let mut steps = failed_refresh();
        steps.extend([
            Input(Event::Reset(13.0), vec![Cancel]),
            Pending(None),
            Input(Event::Zoom(18.0), vec![Timer(400)]),
            Stale,
            Wait(400),
            Fire(vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push(("finding 1: reset then zoom before FontDue", steps));

        let mut steps = failed_refresh();
        steps.extend([
            Wait(2000), // Deadline passed, but FontDue is still in the queue.
            Input(Event::Zoom(18.0), vec![Cancel, Timer(400)]),
            Stale,
            Wait(400),
            Fire(vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push(("finding 1: zoom after deadline before FontDue", steps));

        let mut steps = failed_refresh();
        for millis in [1000, 2000, 4000, 8000, 16000] {
            steps.extend([
                Wait(millis),
                Fire(vec![Read]),
                Input(BAD, vec![Timer((millis * 2).min(30000))]),
            ]);
        }
        steps.extend([
            Input(Event::Reset(13.0), vec![Cancel]),
            Pending(None),
            Wait(60000),
            Stale,
        ]);
        cases.push((
            "finding 2: reset cancels idle 30 second refresh loop",
            steps,
        ));

        let mut steps = ready();
        steps.extend([
            Input(Event::ReadRequested, vec![Read]),
            Input(
                Event::ReadAnswered {
                    usable: false,
                    following: false,
                    continued: false,
                    retry: None,
                },
                vec![],
            ),
            Input(Event::Zoom(16.0), vec![Timer(400)]),
            Wait(400),
            Fire(vec![Read]),
            Input(BAD, vec![Timer(1000)]),
            Pending(Some(16.0)),
            Wait(1000),
            Fire(vec![Read]),
            Input(GOOD, vec![Write(16.0)]),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Pending(None),
            Input(GOOD, vec![]),
        ]);
        cases.push(("outage retains zoom until reconnected look", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::Zoom(18.0), vec![]),
            Input(Event::ReadRequested, vec![Read]),
            Input(GOOD, vec![]), // A's summary arrives before its receipt.
            Pending(Some(18.0)),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(GOOD, vec![Write(18.0)]), // Unchanged is also a fresh fence.
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(GOOD, vec![]),
            Pending(None),
        ]);
        cases.push(("write A in flight while B arrives", steps));

        let mut steps = ready();
        steps.extend([
            Input(Event::Reset(13.0), vec![]),
            Input(Event::Zoom(16.0), vec![Timer(400)]),
            Input(Event::Reset(13.0), vec![Cancel]),
            Stale,
            Pending(None),
        ]);
        cases.push(("Ctrl+0 discards an unwritten zoom", steps));

        for profile in [13.0, 16.0] {
            let mut steps = writing();
            steps.extend([
                Input(Event::Zoom(18.0), vec![]),
                Input(Event::Reset(profile), vec![]),
                Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
                Input(
                    GOOD,
                    if profile == 16.0 {
                        vec![]
                    } else {
                        vec![Write(profile)]
                    },
                ),
            ]);
            if profile != 16.0 {
                steps.extend([
                    Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
                    Input(GOOD, vec![]),
                ]);
            }
            steps.push(Pending(None));
            cases.push(("Ctrl+0 only corrects a different in-flight size", steps));
        }

        let mut steps = writing();
        steps.extend([
            Input(Event::ReadRequested, vec![Read]),
            Input(Event::WriteSettled(WriteReply::Conflict), vec![]),
            Input(GOOD, vec![Read]), // The pre-conflict read is insufficient.
            Input(Event::ReadRequested, vec![]), // Coalesced hint also finishes first.
            Input(GOOD, vec![Read]),
            Input(GOOD, vec![Write(16.0)]),
        ]);
        cases.push((
            "conflict waits for a fresh fence and coalesced reads",
            steps,
        ));

        let mut steps = failed_refresh();
        steps.extend([
            Input(Event::ReadRequested, vec![]), // Hint cannot bypass backoff.
            Wait(1000),
            Fire(vec![Read]),
            Input(Event::Reset(13.0), vec![]),
            Input(BAD, vec![]), // Already active retry must not reschedule.
            Pending(None),
        ]);
        cases.push((
            "reset during refresh suppresses retry after its answer",
            steps,
        ));

        let mut steps = writing();
        for millis in [1000, 2000, 4000, 8000, 16000, 30000, 30000] {
            steps.extend([
                Input(Event::WriteSettled(WriteReply::Failed), vec![Timer(millis)]),
                Pending(Some(16.0)),
                Wait(millis),
                Fire(vec![Write(16.0)]),
            ]);
        }
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Failed), vec![Timer(30000)]),
            Input(Event::Zoom(18.0), vec![Cancel, Timer(400)]),
            Stale,
            Wait(400),
            Fire(vec![Write(18.0)]),
            Input(Event::WriteSettled(WriteReply::Failed), vec![Timer(1000)]),
            Wait(1000),
            Fire(vec![Write(18.0)]),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(GOOD, vec![]),
            Pending(None),
        ]);
        cases.push((
            "failed writes cap at 30 seconds and new zoom resets backoff",
            steps,
        ));

        let mut steps = writing();
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Failed), vec![Timer(1000)]),
            Wait(1000),
            Fire(vec![Write(16.0)]),
            Input(Event::WriteSettled(WriteReply::Refused), vec![Read]),
            Input(GOOD, vec![]),
            Pending(None),
            Input(Event::TimerFired(0), vec![]),
        ]);
        cases.push(("validation refusal clears pending and retry", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::Zoom(18.0), vec![]),
            Input(Event::WriteSettled(WriteReply::Refused), vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
            Pending(Some(18.0)),
        ]);
        cases.push(("refusal of A does not refuse newer B", steps));

        let mut steps = failed_refresh();
        steps.extend([
            Wait(1000),
            Fire(vec![Read]),
            Input(
                Event::ReadAnswered {
                    usable: false,
                    following: false,
                    continued: true,
                    retry: None,
                },
                vec![Read],
            ),
            Input(GOOD, vec![Write(16.0)]),
        ]);
        cases.push(("retargeted read stays on the fresh fence path", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Failed), vec![Timer(1000)]),
            Wait(2000),
            Fire(vec![Write(16.0)]),
            Input(Event::WriteSettled(WriteReply::Conflict), vec![Read]),
            Input(BAD, vec![Timer(2000)]),
            Input(Event::Zoom(18.0), vec![Cancel, Timer(400)]),
            Wait(400),
            Fire(vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push((
            "conflict after overdue write retry never schedules zero delay",
            steps,
        ));

        for (name, steps) in cases {
            let mut state = Persistence::default();
            let mut now = Instant::now();
            let mut timer = None;
            let mut cancelled = None;
            for (index, step) in steps.into_iter().enumerate() {
                let context = format!("{name}, step {index}, {state:?}");
                let (event, expected) = match step {
                    Input(event, effects) => (Some(event), effects),
                    Wait(millis) => {
                        now += Duration::from_millis(millis);
                        (None, vec![])
                    }
                    Fire(effects) => {
                        let (generation, at) = timer.take().expect(&context);
                        assert!(now >= at, "{context}");
                        (Some(Event::TimerFired(generation)), effects)
                    }
                    Stale => (Some(Event::TimerFired(cancelled.expect(&context))), vec![]),
                    Pending(size) => {
                        assert_eq!(state.pending, size, "{context}");
                        (None, vec![])
                    }
                };
                let actions = event.map_or_else(Vec::new, |event| state.event(event, now));
                let mut effects = Vec::new();
                for action in actions {
                    effects.push(match action {
                        Action::Write(size) => {
                            assert!(!state.refresh && !state.reading, "{context}");
                            Write(size)
                        }
                        Action::Read => {
                            assert!(state.reading, "{context}");
                            if state.persistence_read {
                                assert!(state.pending.is_some(), "{context}");
                            }
                            Read
                        }
                        Action::Schedule { delay, generation } => {
                            assert!(!delay.is_zero() && delay <= MAX_BACKOFF, "{context}");
                            assert!(timer.is_none(), "two live timers: {context}");
                            timer = Some((generation, now + delay));
                            Timer(delay.as_millis() as u64)
                        }
                        Action::CancelTimer => {
                            cancelled = timer.take().map(|(generation, _)| generation);
                            Cancel
                        }
                        Action::RetryLook(delay) => Effect::LookRetry(delay.as_millis() as u64),
                    });
                }
                assert_eq!(effects, expected, "{context}");
                assert_eq!(state.retry_at, timer.map(|(_, at)| at), "{context}");
                assert!(
                    state.pending.is_none()
                        || state.sent.is_some()
                        || state.reading
                        || timer.is_some(),
                    "pending zoom has no live path: {context}"
                );
                if state.pending.is_none() && state.sent.is_none() {
                    assert!(timer.is_none() && !state.refresh, "idle retry: {context}");
                }
                assert!(state.attempts <= 6, "{context}");
                if matches!(
                    event,
                    Some(
                        Event::Zoom(_)
                            | Event::WriteSettled(WriteReply::Saved | WriteReply::Refused)
                    )
                ) || matches!(event, Some(Event::ReadAnswered { usable: true, .. }))
                {
                    assert_eq!(state.attempts, 0, "backoff did not reset: {context}");
                }
            }
        }
    }

    #[test]
    fn conflict_retry_writes_with_the_native_followers_fresh_revision() {
        use settings::follow::{Followed, Follower};

        fn summary(revision: &str) -> String {
            serde_json::json!({"status":"current","summary":{
                "schema":1,"binding":{"instance":"example","profile":"default"},
                "incarnation":"inc","revision":revision,"design_revision":"1",
                "source_digest":"d","appearance":settings::Appearance::default(),
                "custom_source":false
            }})
            .to_string()
        }

        fn answer(
            state: &mut Persistence,
            follower: &mut Follower,
            ticket: u64,
            body: Option<&str>,
            now: Instant,
        ) -> Vec<Action> {
            let (followed, next) = follower.answered(ticket, body.map(|body| (0, body, None)));
            assert!(next.is_none(), "the persistence machine owns coalescing");
            state.event(
                Event::ReadAnswered {
                    usable: matches!(followed, Some(Followed::Look { .. } | Followed::Unchanged)),
                    following: follower.following(),
                    continued: false,
                    retry: follower.retry_delay(),
                },
                now,
            )
        }

        let mut state = Persistence::default();
        let mut follower = Follower::new("example");
        let mut now = Instant::now();
        assert_eq!(state.event(Event::ReadRequested, now), [Action::Read]);
        let read = follower.read().unwrap();
        assert!(
            answer(
                &mut state,
                &mut follower,
                read.ticket,
                Some(&summary("1")),
                now
            )
            .is_empty()
        );
        state.event(Event::Zoom(16.0), now);
        now += QUIET;
        assert_eq!(
            state.event(Event::TimerFired(state.generation), now),
            [Action::Write(16.0)]
        );
        let write = follower
            .apply("first", serde_json::json!({"terminal.font.size":16}))
            .unwrap();
        assert_eq!(write.body["expected_revision"], "1");

        assert_eq!(state.event(Event::ReadRequested, now), [Action::Read]);
        let old = follower.read().unwrap();
        assert!(
            state
                .event(Event::WriteSettled(WriteReply::Conflict), now)
                .is_empty()
        );
        assert_eq!(
            answer(
                &mut state,
                &mut follower,
                old.ticket,
                Some(&summary("1")),
                now
            ),
            [Action::Read]
        );
        let fresh = follower.read().unwrap();
        assert!(
            matches!(answer(&mut state, &mut follower, fresh.ticket, None, now).as_slice(),
            [Action::Schedule { delay, .. }] if *delay == Duration::from_secs(1))
        );

        let old_generation = state.generation;
        state.event(Event::Zoom(18.0), now);
        assert!(
            state
                .event(Event::TimerFired(old_generation), now)
                .is_empty()
        );
        now += QUIET;
        assert_eq!(
            state.event(Event::TimerFired(state.generation), now),
            [Action::Read]
        );
        let read = follower.read().unwrap();
        assert_eq!(
            answer(
                &mut state,
                &mut follower,
                read.ticket,
                Some(&summary("2")),
                now
            ),
            [Action::Write(18.0)]
        );
        let write = follower
            .apply("retry", serde_json::json!({"terminal.font.size":18}))
            .unwrap();
        assert_eq!(write.body["expected_revision"], "2");
        assert_eq!(write.body["expected_incarnation"], "inc");
    }
}
