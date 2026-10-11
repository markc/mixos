// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shell's settings lane: UI-loop activation of the session look. Hosts feed their existing Consumer,
//! prepare captured requests on their existing worker and complete them here.
//! No transport, runtime, receiver, file I/O, timer or redraw loop is owned.
use settings::{
    Diagnostic, Snapshot,
    consumer::{Consumer, Update},
    domains::ChangePlan,
    fallback::PresentationKind,
};
pub mod native;

/// A value taken at most once, by whichever clone gets there first.
pub struct Once<T>(std::sync::Arc<std::sync::Mutex<Option<T>>>);
impl<T> Once<T> {
    pub fn new(value: T) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(value))))
    }
    pub fn take(&self) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
impl<T> Clone for Once<T> {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}
impl<T> PartialEq for Once<T> {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T> Eq for Once<T> {}
impl<T> std::fmt::Debug for Once<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Once").finish_non_exhaustive()
    }
}

pub struct Host<T = ()> {
    consumer: Consumer,
    presentation: Option<Presentation<T>>,
    preparing: Option<Update>,
}
#[derive(Debug)]
pub struct Presentation<T> {
    content: T,
}
#[derive(Clone, Debug)]
pub struct Request {
    update: Update,
    context: String,
}
#[derive(Debug)]
pub struct Completion<T> {
    update: Update,
    result: Box<Result<Presentation<T>, Diagnostic>>,
}

impl<T> Host<T> {
    pub fn new(consumer: Consumer) -> Self {
        Self {
            consumer,
            presentation: None,
            preparing: None,
        }
    }
    pub fn consumer(&self) -> &Consumer {
        &self.consumer
    }
    /// Feed connection, delivery, RPC and retry events to this single consumer.
    pub fn consumer_mut(&mut self) -> &mut Consumer {
        &mut self.consumer
    }
    pub fn presentation(&self) -> Option<&Presentation<T>> {
        self.presentation.as_ref()
    }
    pub fn kind(&self) -> Option<PresentationKind> {
        self.consumer.presentation_kind()
    }
    /// Capture once per pending stage. When a new capture replaces stale work,
    /// the host cancels/replaces its one resource job; this type spawns nothing.
    pub fn request(&mut self) -> Option<Request> {
        if self
            .preparing
            .as_ref()
            .is_some_and(|update| self.consumer.is_current(update))
        {
            return None;
        }
        let update = self.consumer.pending()?.clone();
        self.preparing = Some(update.clone());
        Some(Request {
            update,
            context: self.consumer.context().to_owned(),
        })
    }
    /// Synchronous UI-loop operation. No await or callback can interleave the
    /// final fence, whole presentation replacement and applied acknowledgement.
    /// Returns the shared change plan only for a current successful activation.
    pub fn complete(&mut self, completion: Completion<T>) -> Option<ChangePlan> {
        self.complete_with(completion, |_| {})
    }

    /// Activate caller-owned, already prepared UI state after the final fence
    /// and before acknowledgement. The callback is synchronous and total: it
    /// must not perform I/O, await, or discover unsupported resources.
    /// Use the passed presentation: the host's stored value is replaced after
    /// this callback returns.
    pub fn complete_with(
        &mut self,
        completion: Completion<T>,
        activate: impl FnOnce(&Presentation<T>),
    ) -> Option<ChangePlan> {
        if !self.consumer.is_current(&completion.update) {
            return None;
        }
        self.preparing = None;
        match *completion.result {
            Ok(presentation) => {
                let changes = completion.update.changes();
                activate(&presentation);
                self.presentation = Some(presentation);
                assert!(
                    self.consumer.acknowledge(&completion.update),
                    "synchronous activation fence"
                );
                Some(changes)
            }
            Err(fault) => {
                self.consumer.failed(&completion.update, fault);
                None
            }
        }
    }
}
impl Request {
    pub fn update(&self) -> &Update {
        &self.update
    }
    /// Turn a host worker cancellation, panic or timeout into the same fenced
    /// failure path. Keep a capture outside the job until completion arrives.
    pub fn failed<T>(self, fault: Diagnostic) -> Completion<T> {
        Completion {
            update: self.update,
            result: Box::new(Err(fault)),
        }
    }
    /// Run on a host worker: build the whole presentation from the captured
    /// snapshot before it can be activated.
    pub fn prepare_registered<T>(
        self,
        content: impl FnOnce(&Snapshot) -> Result<T, Diagnostic>,
    ) -> Completion<T> {
        let result = self
            .update
            .snapshot()
            .effective
            .get(&self.context)
            .ok_or_else(|| {
                Diagnostic::new(
                    "missing_context",
                    "effective",
                    "Captured context disappeared",
                )
            })
            .and_then(|_| content(self.update.snapshot()))
            .map(|content| Presentation { content });
        Completion {
            update: self.update,
            result: Box::new(result),
        }
    }
}
impl<T> Presentation<T> {
    pub fn content(&self) -> &T {
        &self.content
    }
}
