// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure Request transitions, called only under the bridge's registry lock.
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub enum Phase {
    Sending,
    Open(String),
    Closing,
    Ended,
}

#[derive(Debug)]
pub enum Event {
    Reply(Option<String>),
    Outcome(Value),
    Close,
    Timeout,
    Lost,
    Sweep,
}

#[derive(Debug, Default)]
pub struct Effects {
    pub response: Option<Value>,
    pub cancel: Option<String>,
    pub unexport: bool,
}

pub struct State {
    pub phase: Phase,
    parked: Vec<Value>,
    /// Close and timeout both suppress any subsequent Response. Closing can
    /// remain in the table after its D-Bus object has been removed.
    answered: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Sending,
            parked: Vec::new(),
            answered: false,
        }
    }
}

impl State {
    pub fn apply(&mut self, event: Event) -> Effects {
        let mut effects = Effects::default();
        match (&self.phase, event) {
            (Phase::Ended, _) => return effects,
            (Phase::Sending, Event::Outcome(value)) => {
                // Only outcomes with this entry's correlation reach here.
                if self.parked.len() < 64 {
                    self.parked.push(value);
                }
            }
            (Phase::Open(id), Event::Outcome(value)) if value["request"].as_str() == Some(id) => {
                effects.response = Some(value);
                self.phase = Phase::Ended;
            }
            (Phase::Sending, Event::Reply(Some(id))) => {
                let outcome = self
                    .parked
                    .iter()
                    .find(|v| v["request"].as_str() == Some(&id))
                    .cloned();
                self.parked.clear();
                self.phase = Phase::Open(id);
                if let Some(outcome) = outcome {
                    return self.apply(Event::Outcome(outcome));
                }
            }
            (Phase::Closing, Event::Reply(Some(id))) => {
                effects.cancel = Some(id);
                self.phase = Phase::Ended;
            }
            (Phase::Sending, Event::Close) => {
                self.phase = Phase::Closing;
                effects.unexport = true;
            }
            (Phase::Open(id), Event::Close) => {
                effects.cancel = Some(id.clone());
                self.phase = Phase::Ended;
                effects.unexport = true;
            }
            (Phase::Sending, Event::Timeout) => {
                self.phase = Phase::Closing;
                effects.response = Some(serde_json::json!({"response": 2}));
            }
            (_, Event::Lost) | (Phase::Sending | Phase::Closing, Event::Reply(None)) => {
                if let Phase::Open(id) = &self.phase {
                    effects.cancel = Some(id.clone());
                }
                self.phase = Phase::Ended;
                if !self.answered {
                    effects.response = Some(serde_json::json!({"response": 2}));
                }
            }
            (Phase::Closing, Event::Sweep) => {
                self.phase = Phase::Ended;
            }
            _ => {}
        }
        if self.answered {
            effects.response = None;
        }
        if effects.response.is_some() || effects.unexport {
            self.answered = true;
            effects.unexport = true;
        }
        if self.phase == Phase::Ended {
            self.parked.clear();
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn permutations(items: &mut [u8], start: usize, check: &mut impl FnMut(&[u8])) {
        if start == items.len() {
            check(items);
            return;
        }
        for i in start..items.len() {
            items.swap(start, i);
            permutations(items, start + 1, check);
            items.swap(start, i);
        }
    }

    #[test]
    fn every_order_of_reply_outcome_close_disconnect_loss_and_timeout() {
        // Caller disconnect is the same transition as the owner's Close.
        for refusal in [false, true] {
            permutations(&mut [0, 1, 2, 3, 4, 5], 0, &mut |order| {
                let mut state = State::default();
                let mut responses = 0;
                let mut closed = false;
                for event in order {
                    let event = match event {
                        0 => Event::Reply((!refusal).then(|| "inc-1".into())),
                        1 => Event::Outcome(json!({"request":"inc-1", "response":0})),
                        2 | 3 => {
                            closed = true;
                            Event::Close
                        }
                        4 => Event::Lost,
                        _ => Event::Timeout,
                    };
                    let effects = state.apply(event);
                    if effects.response.is_some() {
                        assert!(!closed, "Response after Close: {order:?}");
                        responses += 1;
                    }
                    assert!(responses <= 1, "duplicate Response: {order:?}");
                }
                state.apply(Event::Sweep);
                assert_eq!(state.phase, Phase::Ended);
            });
        }
    }

    #[test]
    fn every_completion_order_without_close_or_loss_answers_exactly_once() {
        for refusal in [false, true] {
            permutations(&mut [0, 1, 2], 0, &mut |order| {
                let mut state = State::default();
                let mut responses = Vec::new();
                for event in order {
                    let effects = state.apply(match event {
                        0 => Event::Reply((!refusal).then(|| "inc-1".into())),
                        1 => Event::Outcome(json!({"request":"inc-1", "response":0})),
                        _ => Event::Timeout,
                    });
                    if let Some(response) = effects.response {
                        responses.push(response);
                    }
                }
                assert_eq!(responses.len(), 1, "missing/duplicate Response: {order:?}");
                if refusal {
                    assert_eq!(responses[0]["response"], 2);
                }
                state.apply(Event::Sweep);
                assert_eq!(state.phase, Phase::Ended);
            });
        }
        let mut state = State::default();
        assert_eq!(
            state.apply(Event::Reply(None)).response.unwrap()["response"],
            2
        );
        assert!(state.apply(Event::Reply(None)).response.is_none());
    }

    #[test]
    fn provider_loss_cancels_an_open_request() {
        let mut state = State::default();
        state.apply(Event::Reply(Some("inc-1".into())));
        let effects = state.apply(Event::Lost);
        assert_eq!(effects.cancel.as_deref(), Some("inc-1"));
        assert_eq!(effects.response.unwrap()["response"], 2);
        assert!(effects.unexport);
        assert!(state.apply(Event::Lost).cancel.is_none());
    }

    #[test]
    fn only_reply_request_can_consume_a_parked_outcome() {
        let mut state = State::default();
        state.apply(Event::Outcome(json!({"request":"old", "response":0})));
        assert!(
            state
                .apply(Event::Reply(Some("new".into())))
                .response
                .is_none()
        );
        assert!(
            state
                .apply(Event::Outcome(json!({"request":"old", "response":0})))
                .response
                .is_none()
        );
        assert!(
            state
                .apply(Event::Outcome(json!({"request":"new", "response":0})))
                .response
                .is_some()
        );
    }

    #[test]
    fn close_and_timeout_cancel_late_reply_but_never_answer_it() {
        for event in [Event::Close, Event::Timeout] {
            let mut state = State::default();
            state.apply(event);
            let effects = state.apply(Event::Reply(Some("inc-1".into())));
            assert_eq!(effects.cancel.as_deref(), Some("inc-1"));
            assert!(effects.response.is_none());
            assert_eq!(state.phase, Phase::Ended);
        }
    }
}
