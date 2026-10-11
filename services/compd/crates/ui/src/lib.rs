// SPDX-License-Identifier: MIT OR Apache-2.0
//! The compositor's own UI: egui programs drawn by compd itself (capture
//! overlays and dialogs, notifications, the frame-rate overlay, the shell).
//!
//! Each instance is an [`engine::UiRuntime`] painting into a GL texture the
//! compositor owns, through egui-wgpu on the wgpu device that shares compd's
//! EGL context. The compositor composites that texture like any other
//! element, so the UI costs one texture sample per frame and nothing at all
//! while it is idle.

#[macro_use]
extern crate model;

pub mod engine;

pub use engine::{MessageHandler, UiProgram, UiRuntime, UiSnapshot};
pub use graphics::surface::{WgpuGlContext, create_wgpu_gl_context, make_current, texture_format};
