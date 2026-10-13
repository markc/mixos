// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The capture overlays and dialogs, egui programs on the MixOS look.
//!
//! All are screen-space instances driven by the capture interface:
//! - [`setup::SetupOverlay`]   — the dim mask + passthrough hole + target/
//!   media chooser + region drag (the setup phase).
//! - [`border::RegionBorder`]  — the bright outline around the captured region
//!   (drawn above everything).
//! - [`dim::RegionDim`]        — the dark backdrop with a clear hole (drawn
//!   below windows).
//! - [`hud::StopHud`]          — the top-right Stop button shown while capturing.
//! - [`dialog::ContinueDialog`]— the 5-minute "continue?" countdown dialog.
//!
//! Every UI's `UiProgram::Message` is
//! [`crate::capture::session::message::CaptureMessage`].

pub mod border;
pub mod dialog;
pub mod dim;
pub mod encodialog;
pub mod errordialog;
pub mod hud;
pub mod mask;
pub mod savedialog;
pub mod setup;
pub mod style;
