// SPDX-License-Identifier: MIT OR Apache-2.0
//! Composition belongs to a pane, including while its preedit is empty.
//!
//! egui reports an input method as preedit and commit events only. The
//! window allows the input method while a pane has the keyboard; a focus
//! change while composing disallows it until the input method acknowledges
//! with an empty preedit, so a composition started in one pane can never
//! commit into another.

/// Frames a reset may wait for its acknowledgement before the input method
/// is allowed again anyway (one with nothing composed sends none).
const RESET_FRAMES: u8 = 2;

#[derive(Default)]
pub struct Composition {
    /// The pane a composition in progress belongs to.
    owner: Option<u64>,
    /// A non-empty preedit is showing.
    composing: bool,
    /// Disallowed after a focus change, until acknowledged.
    resetting: bool,
    /// Frames since the reset began.
    waited: u8,
    /// A reset was acknowledged this frame: its stale commit may follow.
    settled: bool,
}

impl Composition {
    /// Whether the input method may be allowed this frame.
    pub fn enabled(&self) -> bool {
        !self.resetting
    }

    /// Call once at the start of every frame.
    pub fn frame(&mut self) {
        self.settled = false;
        if self.resetting {
            self.waited += 1;
            if self.waited > RESET_FRAMES {
                self.resetting = false;
            }
        }
    }

    pub fn has_owner(&self) -> bool {
        self.owner.is_some()
    }

    /// A preedit for the pane `active` has the keyboard: whether to show it.
    pub fn preedit(&mut self, text: &str, active: Option<u64>) -> bool {
        if self.focus(active) {
            return false;
        }
        if text.is_empty() {
            // The input method's acknowledgement of a reset, or the end of
            // a composition (a commit usually follows).
            if self.resetting {
                self.resetting = false;
                self.settled = true;
            }
            self.composing = false;
            return false;
        }
        if self.resetting || self.settled || active.is_none() {
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
            return false;
        }
        let accepted = !self.resetting
            && !self.settled
            && active.is_some()
            && self.owner.is_none_or(|owner| Some(owner) == active);
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

    /// Abandon any composition: the window disallows the input method until
    /// it acknowledges, so queued preedits and commits cannot attach to the
    /// newly focused pane.
    pub fn cancel(&mut self) {
        if self.composing || self.owner.is_some() {
            self.resetting = true;
            self.waited = 0;
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
            assert!(!ime.enabled());
            assert!(!ime.commit(next));
            assert!(!ime.preedit("b", next));
            assert!(!ime.commit(Some(1)), "even after switching back");
            // The input method acknowledges; its stale commit may follow in
            // the same frame and is dropped.
            assert!(!ime.preedit("", next));
            assert!(ime.enabled());
            assert!(!ime.commit(Some(2)));
            ime.frame();
            assert!(ime.preedit("c", Some(2)));
            assert!(ime.commit(Some(2)));
        }
    }

    #[test]
    fn commit_checks_owner_even_before_focus_notification() {
        let mut ime = Composition::default();
        assert!(ime.preedit("a", Some(1)));
        assert!(!ime.commit(Some(2)));
        assert!(!ime.enabled());
        ime.preedit("", Some(2));
        ime.frame();
        assert!(ime.commit(Some(2)), "a direct commit without preedit");
        assert!(ime.preedit("b", Some(2)));
        assert!(ime.preedit("bc", Some(2)));
        // The end of a composition: an empty preedit, then the commit.
        assert!(!ime.preedit("", Some(2)));
        assert!(ime.commit(Some(2)));
    }

    #[test]
    fn a_reset_with_no_acknowledgement_ends_after_a_few_frames() {
        let mut ime = Composition::default();
        assert!(ime.preedit("a", Some(1)));
        ime.cancel();
        assert!(!ime.enabled());
        for _ in 0..RESET_FRAMES {
            ime.frame();
            assert!(!ime.enabled());
        }
        ime.frame();
        assert!(ime.enabled());
        // Cancelling with nothing composed never disallows it.
        ime.cancel();
        assert!(ime.enabled());
    }
}
