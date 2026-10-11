// SPDX-License-Identifier: MIT OR Apache-2.0
//! The compositor's own UI: egui programs drawn by compd itself (capture
//! overlays and dialogs, notifications, the frame-rate overlay, the shell).
//!
//! Each instance is an [`engine::UiRuntime`] painting into a GL texture the
//! compositor owns, through egui-wgpu on the wgpu device that shares compd's
//! EGL context. The compositor composites that texture like any other
//! element, so an idle UI costs nothing and a visible one one texture sample.
//!
//! ## Public types
//!
//! - [`UiRegistry`]: every instance of one world. It stores no camera:
//!   render and hit-test calls take a [`Transform`] and the output size, as
//!   smithay's `window.render_elements` takes position and scale.
//! - [`UiItem`]: a type-erased instance; typed access through `get::<U>()`.
//! - [`UiInstance<U>`], [`UiHandle<U>`]: typed access to one program.
//! - [`UiSpace`]: whether an item lives in world or screen coordinates.
//! - [`input::Input`]: what the compositor feeds an instance.
//!
//! ## Per frame (inside the scene's GPU phase)
//!
//! ```ignore
//! let elements = registry.render_all(render_node, gles, transform, output_size, layers);
//! ```
//!
//! ## Input
//!
//! ```ignore
//! if let Some(handle) = registry.dispatch_pointer_at(point, &transform, output_size) {
//!     // the pointer is over a UI item
//! }
//! ```

#[macro_use]
extern crate model;

pub mod element;
pub mod engine;
pub mod error;
pub mod frame_trace;
pub mod handle;
pub mod input;
pub mod instance;
pub mod registry;
pub mod source;
pub mod space;

pub use element::UiRenderElement;
pub use engine::{Gpu, MessageHandler, UiProgram, UiRuntime, UiSnapshot};
pub use error::{CreateError, DispatchError, ResizeError};
pub use handle::{HandleId, UiHandle};
pub use instance::{UiInstance, UiItem};
pub use registry::{PaneView, UiRegistry};
pub use space::{Transform, UiSpace};

pub use graphics::surface::{WgpuGlContext, create_wgpu_gl_context, make_current, texture_format};
