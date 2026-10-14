// SPDX-License-Identifier: MIT OR Apache-2.0
//! Zoom persistence decisions. The shell supplies time and performs actions.

use std::time::{Duration, Instant};

pub(crate) const QUIET: Duration = Duration::from_millis(400);

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
    },
    TimerFired,
    /// A native repaint can deliver the debounce before the Bus is ready.
    Now,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Write(f32),
    Read,
    Debounce,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Persistence {
    pub(crate) pending: Option<f32>,
    pub(crate) unsaved: bool,
    sent: Option<f32>,
    fence_ready: bool,
    reading: bool,
    read_again: bool,
    debounce_at: Option<Instant>,
}

impl Persistence {
    fn read(&mut self, actions: &mut Vec<Action>) {
        if self.reading || self.sent.is_some() {
            self.read_again = true;
        } else {
            self.read_again = false;
            self.reading = true;
            actions.push(Action::Read);
        }
    }

    fn advance(&mut self, actions: &mut Vec<Action>) {
        if self.sent.is_some() || self.reading {
            return;
        }
        if self.read_again {
            self.read(actions);
        } else if let Some(size) = self.pending
            && self.debounce_at.is_none()
        {
            if self.fence_ready {
                self.sent = Some(size);
                actions.push(Action::Write(size));
            } else {
                self.read(actions);
            }
        }
    }

    pub(crate) fn event(&mut self, event: Event, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        match event {
            Event::Zoom(size) => {
                self.pending = Some(size);
                self.debounce_at = Some(now + QUIET);
                actions.push(Action::Debounce);
            }
            Event::Reset(profile_size) => {
                self.pending = self
                    .sent
                    .filter(|size| *size != profile_size)
                    .map(|_| profile_size);
                self.unsaved = false;
                self.debounce_at = None;
                self.advance(&mut actions);
            }
            Event::WriteSettled(reply) => {
                let Some(sent) = self.sent.take() else {
                    return actions;
                };
                match reply {
                    WriteReply::Saved => {
                        self.unsaved = false;
                        if self.pending == Some(sent) {
                            self.pending = None;
                        }
                        // The receipt has no fence. Read the committed revision
                        // before writing a newer zoom or a Ctrl+0 correction.
                        self.fence_ready = false;
                        self.read(&mut actions);
                    }
                    WriteReply::Conflict => {
                        self.fence_ready = false;
                        if self.pending.is_some() {
                            self.read(&mut actions);
                        }
                    }
                    WriteReply::Refused => {
                        self.pending = None;
                        self.debounce_at = None;
                        self.unsaved = true;
                        self.read(&mut actions);
                    }
                    WriteReply::Failed => {
                        self.unsaved = true;
                        self.fence_ready = false;
                        self.debounce_at = None;
                        // A queued hint may read, but the failure itself never
                        // retries, even when a newer zoom arrived in flight.
                        if self.read_again {
                            self.read(&mut actions);
                        }
                    }
                }
            }
            Event::ReadRequested => self.read(&mut actions),
            Event::ReadAnswered {
                usable,
                following,
                continued,
            } => {
                if !self.reading {
                    return actions;
                }
                self.fence_ready = usable && following;
                if continued {
                    actions.push(Action::Read);
                    return actions;
                }
                self.reading = false;
                if usable {
                    self.advance(&mut actions);
                } else {
                    self.debounce_at = None;
                    self.unsaved |= self.pending.is_some();
                    if self.read_again {
                        self.read(&mut actions);
                    }
                }
            }
            Event::TimerFired | Event::Now => {
                if self.debounce_at.is_some_and(|at| now >= at) {
                    self.debounce_at = None;
                    self.advance(&mut actions);
                }
            }
        }
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
    };
    const BAD: Event = Event::ReadAnswered {
        usable: false,
        // A lost reply can leave the follower's old fence cached.
        following: true,
        continued: false,
    };

    enum Step {
        Input(Event, Vec<Action>),
        Wait(u64),
        Pending(Option<f32>),
    }

    fn ready() -> Vec<Step> {
        vec![
            Step::Input(Event::ReadRequested, vec![Action::Read]),
            Step::Input(GOOD, vec![]),
        ]
    }

    fn writing() -> Vec<Step> {
        let mut steps = ready();
        steps.extend([
            Step::Input(Event::Zoom(16.0), vec![Action::Debounce]),
            Step::Wait(400),
            Step::Input(Event::TimerFired, vec![Action::Write(16.0)]),
        ]);
        steps
    }

    #[test]
    fn persistence_sequences_obey_event_driven_retry_policy() {
        use Action::{Debounce, Read, Write};
        use Step::{Input, Pending, Wait};
        let mut cases = Vec::new();

        let mut steps = ready();
        steps.extend([
            Input(Event::Zoom(16.0), vec![Debounce]),
            Wait(200),
            Input(Event::Zoom(18.0), vec![Debounce]),
            Wait(200),
            Input(Event::TimerFired, vec![]), // The first debounce woke early.
            Wait(200),
            Input(Event::Now, vec![Write(18.0)]),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(GOOD, vec![]),
            Pending(None),
        ]);
        cases.push(("only the last zoom's 400 ms debounce writes", steps));

        let mut steps = ready();
        steps.extend([
            Input(Event::ReadRequested, vec![Read]),
            Input(Event::Zoom(16.0), vec![Debounce]),
            Input(GOOD, vec![]),
            Wait(400),
            Input(Event::TimerFired, vec![Write(16.0)]),
            Input(Event::Zoom(18.0), vec![Debounce]),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(GOOD, vec![]),
            Wait(400),
            Input(Event::TimerFired, vec![Write(18.0)]),
        ]);
        cases.push(("read and write answers respect the last zoom debounce", steps));

        for trigger in ["look refresh", "settings hint", "reconnect"] {
            let steps = vec![
                Input(Event::Zoom(16.0), vec![Debounce]),
                Wait(400),
                Input(Event::Now, vec![Read]), // Native repaint, no Bus yet.
                Input(BAD, vec![]),
                Pending(Some(16.0)),
                Wait(60000),
                Input(Event::TimerFired, vec![]), // Early wake-up with no timer.
                Input(Event::Now, vec![]),
                Input(Event::ReadRequested, vec![Read]),
                Input(GOOD, vec![Write(16.0)]),
                Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
                Input(GOOD, vec![]),
                Pending(None),
            ];
            cases.push((trigger, steps));
        }

        let mut steps = writing();
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Failed), vec![]),
            Pending(Some(16.0)),
            Input(Event::Now, vec![]),
            Input(Event::Zoom(18.0), vec![Debounce]),
            Wait(400),
            Input(Event::TimerFired, vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push(("zoom key retries a failed write after debounce", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::Zoom(18.0), vec![Debounce]),
            Wait(400),
            Input(Event::TimerFired, vec![]),
            Input(Event::WriteSettled(WriteReply::Failed), vec![]),
            Pending(Some(18.0)),
            Wait(60000),
            Input(Event::TimerFired, vec![]),
            Input(Event::Now, vec![]),
            Input(Event::ReadRequested, vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push((
            "newer zoom in flight then failure waits for next event",
            steps,
        ));

        let mut steps = writing();
        steps.extend([
            Input(Event::Zoom(18.0), vec![Debounce]),
            Input(Event::WriteSettled(WriteReply::Failed), vec![]),
            Wait(400),
            Input(Event::TimerFired, vec![]),
            Pending(Some(18.0)),
            Input(Event::Zoom(20.0), vec![Debounce]),
            Wait(400),
            Input(Event::TimerFired, vec![Read]),
            Input(GOOD, vec![Write(20.0)]),
        ]);
        cases.push(("failure cancels even a newer undelivered debounce", steps));

        let mut steps = ready();
        steps.extend([
            Input(Event::Zoom(16.0), vec![Debounce]),
            Input(Event::Reset(13.0), vec![]),
            Wait(400),
            Input(Event::TimerFired, vec![]),
            Pending(None),
        ]);
        cases.push(("Ctrl+0 discards an unwritten zoom", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Failed), vec![]),
            Input(Event::Reset(13.0), vec![]),
            Pending(None),
            Input(Event::ReadRequested, vec![Read]),
            Input(GOOD, vec![]),
        ]);
        cases.push(("Ctrl+0 drops failed pending zoom without writing", steps));

        for profile in [13.0, 16.0] {
            let mut steps = writing();
            steps.extend([
                Input(Event::Zoom(18.0), vec![Debounce]),
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
            Input(Event::ReadRequested, vec![]), // A hint queues behind A.
            Input(Event::Zoom(18.0), vec![Debounce]),
            Wait(400),
            Input(Event::TimerFired, vec![]),
            Input(Event::WriteSettled(WriteReply::Saved), vec![Read]),
            Input(Event::ReadRequested, vec![]), // Coalesce another hint.
            Input(GOOD, vec![Read]),
            Input(GOOD, vec![Write(18.0)]),
        ]);
        cases.push(("outstanding requests settle before the next write", steps));

        let mut steps = writing();
        steps.extend([
            Input(Event::WriteSettled(WriteReply::Conflict), vec![Read]),
            Input(BAD, vec![]),
            Pending(Some(16.0)),
            Input(Event::TimerFired, vec![]),
            Input(Event::ReadRequested, vec![Read]),
            Input(GOOD, vec![Write(16.0)]),
        ]);
        cases.push((
            "conflict reads fresh fence; failed read waits for event",
            steps,
        ));

        let mut steps = writing();
        steps.extend([
            Input(Event::ReadRequested, vec![]),
            Input(Event::WriteSettled(WriteReply::Failed), vec![Read]),
            Input(BAD, vec![]),
            Input(Event::Now, vec![]),
            Input(Event::ReadRequested, vec![Read]),
            Input(GOOD, vec![Write(16.0)]),
        ]);
        cases.push(("queued hint may read after failure but cannot write", steps));

        for newer in [false, true] {
            let mut steps = writing();
            if newer {
                steps.push(Input(Event::Zoom(18.0), vec![Debounce]));
            }
            steps.extend([
                Input(Event::WriteSettled(WriteReply::Refused), vec![Read]),
                Pending(None),
                Input(GOOD, vec![]),
                Wait(400),
                Input(Event::TimerFired, vec![]),
                Input(Event::ReadRequested, vec![Read]),
                Input(GOOD, vec![]),
            ]);
            cases.push(("validation refusal clears pending zoom", steps));
        }

        let steps = vec![
            Input(Event::Zoom(16.0), vec![Debounce]),
            Wait(400),
            Input(Event::TimerFired, vec![Read]),
            Input(
                Event::ReadAnswered {
                    usable: false,
                    following: false,
                    continued: true,
                },
                vec![Read],
            ),
            Input(GOOD, vec![Write(16.0)]),
        ];
        cases.push(("retargeted settings read supplies the fence", steps));

        for (name, steps) in cases {
            let mut state = Persistence::default();
            let mut now = Instant::now();
            let mut failed = false;
            for (index, step) in steps.into_iter().enumerate() {
                let context = format!("{name}, step {index}, {state:?}");
                let sent_before = state.sent;
                let reading_before = state.reading;
                let debounce_before = state.debounce_at;
                let event = match step {
                    Input(event, expected) => {
                        if matches!(
                            event,
                            Event::Zoom(_)
                                | Event::Reset(_)
                                | Event::ReadAnswered { usable: true, .. }
                        ) {
                            failed = false;
                        }
                        let actions = state.event(event, now);
                        assert_eq!(actions, expected, "{context}");
                        for action in actions {
                            match action {
                                Debounce => {
                                    assert!(matches!(event, Event::Zoom(_)), "{context}");
                                    assert_eq!(state.debounce_at, Some(now + QUIET), "{context}");
                                }
                                Write(_) => {
                                    assert!(
                                        sent_before.is_none(),
                                        "write still outstanding: {context}"
                                    );
                                    assert!(
                                        !reading_before
                                            || matches!(event, Event::ReadAnswered { .. }),
                                        "read still outstanding: {context}"
                                    );
                                    assert!(!state.reading && state.sent.is_some(), "{context}");
                                    assert!(
                                        !failed,
                                        "write after failure without an event: {context}"
                                    );
                                }
                                Read => assert!(state.reading && state.sent.is_none(), "{context}"),
                            }
                        }
                        Some(event)
                    }
                    Wait(millis) => {
                        now += Duration::from_millis(millis);
                        None
                    }
                    Pending(size) => {
                        assert_eq!(state.pending, size, "{context}");
                        None
                    }
                };
                if let Some(at) = state.debounce_at {
                    assert!(
                        matches!(event, Some(Event::Zoom(_))) || Some(at) == debounce_before,
                        "timer created without zoom key: {context}"
                    );
                }
                if matches!(
                    event,
                    Some(Event::WriteSettled(
                        WriteReply::Failed | WriteReply::Refused
                    )) | Some(Event::ReadAnswered {
                        usable: false,
                        continued: false,
                        ..
                    })
                ) {
                    failed = true;
                    assert!(
                        state.debounce_at.is_none(),
                        "failure retained timer: {context}"
                    );
                }
                if matches!(
                    event,
                    Some(Event::WriteSettled(
                        WriteReply::Failed | WriteReply::Refused
                    ))
                ) {
                    assert!(state.unsaved, "missing unsaved status: {context}");
                }
                if matches!(event, Some(Event::WriteSettled(WriteReply::Refused))) {
                    assert!(
                        state.pending.is_none(),
                        "refusal retained pending: {context}"
                    );
                }
                if let Some(Event::Reset(profile)) = event {
                    assert_eq!(
                        state.pending,
                        sent_before.filter(|size| *size != profile).map(|_| profile),
                        "{context}"
                    );
                    assert!(state.debounce_at.is_none(), "{context}");
                }
                if matches!(event, Some(Event::WriteSettled(WriteReply::Conflict))) {
                    assert!(
                        state.reading && state.sent.is_none() && !state.fence_ready,
                        "{context}"
                    );
                }
                assert!(
                    !(state.reading && state.sent.is_some()),
                    "overlapping requests: {context}"
                );
                // Pending with no request or timer is deliberately valid.
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
        assert_eq!(state.event(Event::TimerFired, now), [Action::Write(16.0)]);
        let write = follower
            .apply("first", serde_json::json!({"terminal.font.size":16}))
            .unwrap();
        assert_eq!(write.body["expected_revision"], "1");

        assert!(state.event(Event::ReadRequested, now).is_empty());
        assert_eq!(
            state.event(Event::WriteSettled(WriteReply::Conflict), now),
            [Action::Read]
        );
        let fresh = follower.read().unwrap();
        assert!(answer(&mut state, &mut follower, fresh.ticket, None, now).is_empty());
        assert!(
            state
                .event(Event::Now, now + Duration::from_secs(60))
                .is_empty()
        );
        assert_eq!(state.event(Event::ReadRequested, now), [Action::Read]);
        let read = follower.read().unwrap();
        assert_eq!(
            answer(
                &mut state,
                &mut follower,
                read.ticket,
                Some(&summary("2")),
                now
            ),
            [Action::Write(16.0)]
        );
        let write = follower
            .apply("retry", serde_json::json!({"terminal.font.size":16}))
            .unwrap();
        assert_eq!(write.body["expected_revision"], "2");
        assert_eq!(write.body["expected_incarnation"], "inc");
    }
}
