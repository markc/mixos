// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Generation-owned, cancellation-safe broadcast conditions. Registration is
//! synchronous, before the evaluator releases its Class C read permit.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tokio::sync::oneshot;

pub(crate) const LIMIT: usize = 1024;
pub(crate) const MAX_TIMEOUT: f64 = 300.0;

#[derive(Default)]
pub(crate) struct Conditions {
    next: u64,
    closed: bool,
    waiters: HashMap<u64, (String, oneshot::Sender<bool>)>,
}

pub(crate) struct Waiter {
    owner: Rc<RefCell<Conditions>>,
    id: u64,
    receiver: oneshot::Receiver<bool>,
}

impl Conditions {
    pub(crate) fn register(owner: &Rc<RefCell<Self>>, key: &str) -> crate::MixResult<Waiter> {
        let mut state = owner.borrow_mut();
        if state.waiters.len() >= LIMIT {
            return Err(crate::native_events::refusal(
                "COND_WAIT_LIMIT",
                "at most 1024 live condition waiters per evaluator generation",
            ));
        }
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        let (sender, receiver) = oneshot::channel();
        if state.closed {
            let _ = sender.send(false);
        } else {
            state.waiters.insert(id, (key.to_owned(), sender));
        }
        Ok(Waiter {
            owner: Rc::clone(owner),
            id,
            receiver,
        })
    }

    pub(crate) fn notify(&mut self, key: &str) -> usize {
        let ids: Vec<_> = self
            .waiters
            .iter()
            .filter(|(_, (k, _))| k == key)
            .map(|(id, _)| *id)
            .collect();
        let mut woke = 0;
        for id in ids {
            if let Some((_, sender)) = self.waiters.remove(&id) {
                woke += usize::from(sender.send(true).is_ok());
            }
        }
        woke
    }

    #[cfg(any(test, feature = "tokio-sleep"))]
    pub(crate) fn shutdown(&mut self) {
        self.closed = true;
        for (_, (_, sender)) in self.waiters.drain() {
            let _ = sender.send(false);
        }
    }
}

impl Waiter {
    pub(crate) async fn wait(mut self, timeout: std::time::Duration) -> bool {
        tokio::time::timeout(timeout, &mut self.receiver)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.owner.borrow_mut().waiters.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn broadcast_registration_precedes_yield_and_does_not_store_permits() {
        let owner = Rc::new(RefCell::new(Conditions::default()));
        assert_eq!(owner.borrow_mut().notify("rev"), 0);
        let a = Conditions::register(&owner, "rev").unwrap();
        let b = Conditions::register(&owner, "rev").unwrap();
        let c = Conditions::register(&owner, "other").unwrap();
        // The next invocation can notify even before either future is polled.
        assert_eq!(owner.borrow_mut().notify("rev"), 2);
        assert!(a.wait(Duration::from_secs(1)).await);
        assert!(b.wait(Duration::from_secs(1)).await);
        assert!(!c.wait(Duration::ZERO).await);
        assert_eq!(owner.borrow_mut().notify("rev"), 0);
        assert!(owner.borrow().waiters.is_empty());
    }

    #[tokio::test]
    async fn shutdown_timeout_cap_and_cancellation() {
        let owner = Rc::new(RefCell::new(Conditions::default()));
        let mut waiters = Vec::new();
        for _ in 0..LIMIT {
            waiters.push(Conditions::register(&owner, "rev").unwrap());
        }
        assert!(Conditions::register(&owner, "rev").is_err());
        drop(waiters.pop());
        let last = Conditions::register(&owner, "rev").unwrap();
        owner.borrow_mut().shutdown();
        assert!(!last.wait(Duration::from_secs(1)).await);
        for waiter in waiters {
            assert!(!waiter.wait(Duration::from_secs(1)).await);
        }
        assert!(
            !Conditions::register(&owner, "rev")
                .unwrap()
                .wait(Duration::from_secs(1))
                .await
        );
        assert!(owner.borrow().waiters.is_empty());
    }
}
