// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Composition belongs to a pane, including while its preedit is empty.
//!
//! egui reports an input method as preedit and commit events only. When the
//! keyboard moves to another pane while a composition is in progress, the
//! window asks the platform to interrupt it, and the cancelled composition
//! stays marked as stale until the platform acknowledges: an empty preedit
//! (the input method reset), or the stale composition's own commit, which is
//! dropped. No amount of time ends it, so a delayed commit can never land in
//! the newly focused pane. Until then a new composition's preedit is hidden.

#[derive(Default)]
pub struct Composition {
    /// The pane a composition in progress belongs to.
    owner: Option<u64>,
    /// A non-empty preedit is showing.
    composing: bool,
    /// A cancelled composition has not been acknowledged yet: its commit,
    /// whenever it comes, is dropped.
    stale: bool,
    /// Ask the platform to interrupt the composition (one frame's output).
    interrupt: bool,
}

impl Composition {
    pub fn has_owner(&self) -> bool {
        self.owner.is_some()
    }

    /// Whether a cancelled composition still waits for the platform.
    pub fn stale(&self) -> bool {
        self.stale
    }

    /// Whether this frame's input method output interrupts composition.
    pub fn take_interrupt(&mut self) -> bool {
        std::mem::take(&mut self.interrupt)
    }

    /// A preedit while the pane `active` has the keyboard: whether to show
    /// it.
    pub fn preedit(&mut self, text: &str, active: Option<u64>) -> bool {
        if self.focus(active) {
            return false;
        }
        if text.is_empty() {
            // The platform's reset acknowledgement, or the end of a
            // composition (a commit usually follows).
            self.stale = false;
            self.composing = false;
            return false;
        }
        if self.stale || active.is_none() {
            return false;
        }
        if self.owner.is_none() {
            self.owner = active;
        }
        self.composing = true;
        self.owner == active
    }

    /// A commit while `active` has the keyboard: whether to send it.
    pub fn commit(&mut self, active: Option<u64>) -> bool {
        if self.focus(active) {
            // The composition was another pane's: this is its commit.
            self.stale = false;
            return false;
        }
        if std::mem::take(&mut self.stale) {
            // The cancelled composition's commit, however late.
            self.owner = None;
            self.composing = false;
            return false;
        }
        let accepted = active.is_some() && self.owner.is_none_or(|owner| Some(owner) == active);
        self.owner = None;
        self.composing = false;
        accepted
    }

    /// The keyboard went to `active`: cancels a composition owned by
    /// another pane, returning true.
    pub fn focus(&mut self, active: Option<u64>) -> bool {
        if self.owner.is_some() && self.owner != active {
            self.cancel();
            true
        } else {
            false
        }
    }

    /// Abandon any composition in progress: the platform is asked to
    /// interrupt it, and its late events cannot attach to another pane.
    pub fn cancel(&mut self) {
        if self.composing || self.owner.is_some() {
            self.stale = true;
            self.interrupt = true;
        }
        self.composing = false;
        self.owner = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_change_close_and_return_cannot_redirect_composition() {
        for next in [Some(2), None] {
            // Other pane or tab, or the owner closed.
            let mut ime = Composition::default();
            assert!(ime.preedit("a", Some(1)));
            assert!(!ime.focus(Some(1)));
            assert!(ime.focus(next));
            assert!(ime.take_interrupt(), "the platform is asked to reset");
            assert!(!ime.take_interrupt());
            assert!(!ime.preedit("b", next), "a stale preedit is hidden");
            assert!(!ime.commit(Some(1)), "even after switching back");
            // That commit was the stale one; the next composition is new.
            assert!(ime.preedit("c", Some(2)));
            assert!(ime.commit(Some(2)));
        }
    }

    /// Review finding: a composition in pane A, the keyboard to pane B, any
    /// number of frames, then A's delayed commit must not land in B.
    #[test]
    fn a_delayed_commit_never_lands_in_the_next_pane() {
        let mut ime = Composition::default();
        assert!(ime.preedit("compose", Some(1)));
        assert!(ime.focus(Some(2)));
        // No frame count, no time: only the platform ends the stale state.
        assert!(ime.stale());
        assert!(!ime.commit(Some(2)), "A's commit, late, in B");
        assert!(!ime.stale());
        assert!(ime.commit(Some(2)), "B's own direct commit");
    }

    #[test]
    fn an_empty_preedit_acknowledges_the_reset() {
        let mut ime = Composition::default();
        assert!(ime.preedit("a", Some(1)));
        ime.cancel();
        assert!(ime.stale() && ime.take_interrupt());
        assert!(!ime.preedit("", Some(2)));
        assert!(!ime.stale());
        assert!(ime.preedit("b", Some(2)));
        // The end of a composition: an empty preedit, then the commit.
        assert!(!ime.preedit("", Some(2)));
        assert!(ime.commit(Some(2)));
        // Cancelling with nothing composed asks for nothing.
        ime.cancel();
        assert!(!ime.stale() && !ime.take_interrupt());
    }

    #[test]
    fn commit_checks_owner_even_before_focus_notification() {
        let mut ime = Composition::default();
        assert!(ime.preedit("a", Some(1)));
        assert!(!ime.commit(Some(2)));
        assert!(!ime.stale(), "that commit was the cancelled one");
        assert!(ime.commit(Some(2)), "a direct commit without preedit");
    }
}
