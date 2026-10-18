// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! # crate::surface
//!
//! The transport layer for rendering wgpu content into textures smithay's GLES
//! renderer composites.
//!
//! wgpu runs on its GL backend over the compositor's
//! OWN EGL context (`wgpu_context`), and UI renders into GL textures that
//! renderer allocates (`surface`, `wgpu_import`). No dmabuf, no second device.
//! `dmabuf_alloc` + `gles_import` remain for screen capture, whose entries the
//! VA-API encoder needs as dmabufs.
//!
//! Knows nothing about the UI toolkit. Sits underneath both `ui::engine`
//! (which renders into wgpu textures provided here) and `ui`
//! (which samples GLES textures provided here).
//!
//! ## Module layering
//!
//! ```text
//! surface.rs        UiSurface  ─┬─ wgpu_import.rs    GlesTexture -> wgpu::Texture (wrap)
//!                                 └─ wgpu_context.rs   WgpuGlContext (GL backend, external context)
//! dmabuf_alloc.rs   AllocatedDmabuf  (capture only)
//! gles_import.rs    Dmabuf -> GlesTexture  (capture only)
//! error.rs          (used by all)
//! ```

pub mod dmabuf_alloc;
pub mod error;
pub mod gles_import;
pub mod surface;
pub mod wgpu_context;
pub mod wgpu_import;

pub use dmabuf_alloc::{AllocatedDmabuf, allocate_dmabuf_negotiated};
pub use error::{AllocError, GlesImportError, SurfaceError, WgpuContextError, WgpuImportError};
pub use gles_import::import_dmabuf_to_gles;
pub use surface::UiSurface;
pub use wgpu_context::{WgpuGlContext, create_wgpu_gl_context, make_current};
pub use wgpu_import::{FORMAT, FOURCC, TEXTURE_USAGE, texture_format, wrap_gles_texture};
