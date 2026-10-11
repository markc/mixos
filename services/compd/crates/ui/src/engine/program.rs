// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contract every compositor-owned UI implements.
//!
//! Deliberately small: no async and no tasks. The compositor drives the
//! lifecycle. A program draws its egui UI each pass and reports what the user
//! did as typed messages; the compositor observes them through the runtime's
//! message handler and may answer with messages of its own.

/// One compositor-owned UI.
///
/// `Send + 'static` so the registry can hold programs behind trait objects.
/// One program type has one message type, so a typed handle can only send
/// that program its own messages.
pub trait UiProgram: Send + 'static {
    type Message: Clone + Send + std::fmt::Debug + 'static;

    /// Draw the UI for this pass. Messages pushed onto `emit` are applied
    /// after the pass, in order, through [`Self::update`].
    fn show(&mut self, ctx: &egui::Context, emit: &mut Vec<Self::Message>);

    /// Apply one message to the program's state.
    fn update(&mut self, message: Self::Message);

    /// Follow-up messages derived from one just applied. They join the same
    /// queue and are applied in order. Default: none.
    fn process(&self, _message: &Self::Message) -> Vec<Self::Message> {
        Vec::new()
    }

    /// Prepare the context once, before the first pass. Default: the MixOS
    /// look the apps use (`toolkit::style::apply` with the embedded theme).
    fn setup(&mut self, ctx: &egui::Context) {
        toolkit::style::apply(ctx, &toolkit::theme::Theme::embedded());
    }
}

/// Opt-in read-back for a program the compositor inspects synchronously.
/// The snapshot is one pass old by construction: fine for reading committed
/// state, wrong for read-modify-write, which should be a message instead.
pub trait UiSnapshot: UiProgram {
    type Snapshot: Send + Sync + Clone + 'static;
    fn snapshot(&self) -> Self::Snapshot;
}
