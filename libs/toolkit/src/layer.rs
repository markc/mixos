// SPDX-License-Identifier: MIT OR Apache-2.0
//! egui programs on wlr-layer-shell surfaces: wallpapers, desktops, docks
//! and overlays (the `layer` feature).
//!
//! [`run`] connects to the compositor and gives the program one surface per
//! output (or one the compositor places, [`Outputs::Compositor`]), each with
//! its own egui context. Every surface:
//!
//! - draws through wgpu's GL backend straight on its `wl_surface`, one
//!   device shared by all of them;
//! - follows its output's fractional scale (`wp_fractional_scale_v1` with
//!   `wp_viewporter`), else the integer buffer scale;
//! - takes pointer, scroll and (when the options ask for it) keyboard input,
//!   and sets the pointer's shape from egui's cursor (`wp_cursor_shape_v1`);
//! - draws only when something changed: a configure, a scale, input, or a
//!   repaint egui asked for. A repaint asked from another thread
//!   (`Context::request_repaint`) wakes the loop, and a delayed one is a
//!   timer, never a poll, so an idle surface costs nothing.
//!
//! Outputs coming and going add and drop surfaces as they do.

mod gpu;
mod input;
mod scale;
mod state;
mod timing;

pub use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer};

/// Which surfaces the program gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outputs {
    /// One per output, added and dropped as outputs come and go.
    Each,
    /// One, on the output the compositor picks.
    Compositor,
}

/// Where the surfaces go and what the compositor is told about them.
#[derive(Clone, Debug)]
pub struct LayerOptions {
    pub layer: Layer,
    /// The namespace the compositor sees.
    pub namespace: String,
    pub keyboard: KeyboardInteractivity,
    pub anchor: Anchor,
    /// -1: under every other surface's exclusive zone (a wallpaper); 0: none;
    /// above 0: reserve that much along the anchored edge (a panel).
    pub exclusive_zone: i32,
    /// The size asked for; 0 on an axis anchored at both ends lets the
    /// compositor size it.
    pub size: (u32, u32),
    pub outputs: Outputs,
}

impl LayerOptions {
    /// A wallpaper or desktop: the Background layer, every output, filling
    /// each, under every exclusive zone, no keyboard.
    pub fn background(namespace: &str) -> Self {
        Self {
            layer: Layer::Background,
            namespace: namespace.to_owned(),
            keyboard: KeyboardInteractivity::None,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: -1,
            size: (0, 0),
            outputs: Outputs::Each,
        }
    }
}

/// What a program sees of the surface a pass draws.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceInfo {
    /// The output's name (`DP-1`), when the surface is on a known one.
    pub output: Option<String>,
    /// Logical size, from the compositor's configure.
    pub width: u32,
    pub height: u32,
    /// Physical pixels per logical pixel (fractional where supported).
    pub scale: f32,
}

/// An egui program drawn on layer surfaces.
pub trait LayerProgram {
    /// Once per surface, on its context, before its first pass: fonts and
    /// style ([`crate::install`]).
    fn setup(&mut self, ctx: &egui::Context);
    /// One pass on one surface.
    fn show(&mut self, ui: &mut egui::Ui, surface: &SurfaceInfo);
    /// A pass on `surface` was painted and presented (a skipped frame calls
    /// nothing).
    fn presented(&mut self, surface: &SurfaceInfo) {
        let _ = surface;
    }
}

/// Run `program` until the connection ends. With [`Outputs::Compositor`] it
/// also ends when the compositor closes the one surface; with
/// [`Outputs::Each`] surfaces come and go with outputs and it keeps running
/// with none (end it with a signal).
pub fn run<P: LayerProgram + 'static>(options: LayerOptions, program: P) -> Result<(), String> {
    state::run(options, program)
}
