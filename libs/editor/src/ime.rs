// SPDX-License-Identifier: MIT OR Apache-2.0
//! The input-method composition. Cancelling one (focus loss or gain, a
//! document switch, a remote edit over it) asks the platform to interrupt it
//! in the next frame's output, and drops preedits and commits until that
//! frame has gone out, so a commit meant for one document never reaches
//! another.
//!
//! A session is open from the first preedit (even an empty one, which some
//! input methods send just before their commit) until its commit.

#[derive(Debug, Default)]
pub struct Composition {
    /// Preedit events arrived and no commit has closed them yet.
    open: bool,
    /// An interrupt is owed to the platform; input is dropped meanwhile.
    resetting: bool,
    /// The preedit text being drawn inline (empty when none).
    pub preedit: String,
    /// The model held a composition range for this preedit at least once, so
    /// its disappearance means a remote edit cancelled it.
    pub anchored: bool,
}

impl Composition {
    /// A preedit update; false when it must be ignored (reset in progress).
    pub fn preedit(&mut self, text: &str) -> bool {
        if self.resetting {
            return false;
        }
        self.open = true;
        self.preedit.clear();
        self.preedit.push_str(text);
        if text.is_empty() {
            self.anchored = false;
        }
        true
    }

    /// A commit; false when it must be dropped (reset in progress).
    pub fn commit(&mut self) -> bool {
        if self.resetting {
            return false;
        }
        self.open = false;
        self.preedit.clear();
        self.anchored = false;
        true
    }

    /// Cancel the composition: the preedit is discarded (the user retypes).
    /// An open session is interrupted.
    pub fn cancel(&mut self) {
        self.resetting |= self.open || self.active();
        self.open = false;
        self.preedit.clear();
        self.anchored = false;
    }

    /// Interrupt whatever the platform is composing, open here or not.
    pub fn interrupt(&mut self) {
        self.cancel();
        self.resetting = true;
    }

    /// Whether this frame's output must interrupt the platform composition.
    /// Taking it ends the reset: the next frame's input is current again.
    pub fn take_interrupt(&mut self) -> bool {
        std::mem::take(&mut self.resetting)
    }

    pub fn active(&self) -> bool {
        !self.preedit.is_empty()
    }

    /// An interrupt is owed: input-method events arriving now belong to the
    /// composition being dropped.
    pub fn resetting(&self) -> bool {
        self.resetting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_drops_queued_input_until_the_interrupt_goes_out() {
        let mut c = Composition::default();
        assert!(c.preedit("ni"));
        c.anchored = true;
        c.cancel();
        assert!(!c.active());
        assert!(
            !c.preedit("nih"),
            "a queued preedit after the cancel is ignored"
        );
        assert!(!c.commit(), "a queued commit after the cancel is dropped");
        assert!(c.take_interrupt());
        assert!(!c.take_interrupt(), "one interrupt per cancel");
        assert!(c.preedit("x"));
        assert!(c.commit());
    }

    #[test]
    fn an_empty_preedit_keeps_the_session_open_until_its_commit() {
        let mut c = Composition::default();
        assert!(c.preedit("ni"));
        assert!(c.preedit(""));
        assert!(!c.active());
        c.cancel();
        assert!(
            !c.commit(),
            "the commit of a session cancelled after Preedit(\"\") is dropped"
        );
        assert!(c.take_interrupt());
    }

    #[test]
    fn cancelling_nothing_owes_no_interrupt_but_interrupt_always_does() {
        let mut c = Composition::default();
        c.cancel();
        assert!(!c.take_interrupt());
        c.interrupt();
        assert!(
            !c.commit(),
            "input during a focus-gain interrupt is dropped"
        );
        assert!(c.take_interrupt());
    }

    #[test]
    fn an_empty_preedit_ends_the_anchor() {
        let mut c = Composition::default();
        assert!(c.preedit("a"));
        c.anchored = true;
        assert!(c.preedit(""));
        assert!(!c.anchored);
        assert!(!c.active());
    }
}
