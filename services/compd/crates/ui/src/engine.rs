// SPDX-License-Identifier: MIT OR Apache-2.0
//! The egui engine: one `UiRuntime<P>` per compositor-owned UI, each with its
//! own egui context and egui-wgpu painter, all on the wgpu device that shares
//! compd's GL context (`graphics::surface::WgpuGlContext`).
//!
//! ```text
//! program.rs   UiProgram    — what a compositor UI implements
//! runtime.rs   UiRuntime<P> — input, passes, repaint requests, paint
//! painter.rs   Painter      — egui-wgpu into a texture view
//! wake.rs      repaint requests → the compositor loop
//! ```

pub mod painter;
pub mod program;
pub mod runtime;
pub mod wake;

pub use painter::Painter;
pub use program::{UiProgram, UiSnapshot};
pub use runtime::{Gpu, MessageHandler, UiRuntime};
pub use wake::register as register_wake;

#[cfg(test)]
mod tests;
