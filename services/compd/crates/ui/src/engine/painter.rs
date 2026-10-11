// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paint one egui pass into a texture view with egui-wgpu.
//!
//! Each runtime owns its painter. egui contexts allocate their managed
//! texture ids independently (the font atlas is id 0 in every one), so one
//! renderer shared between contexts would mix their textures up.
//!
//! The target is the wrapped GL texture of a `graphics::surface::UiSurface`
//! on compd's own context. egui-wgpu wants a non-sRGB target and writes
//! premultiplied alpha, which is what smithay's GLES renderer samples. The
//! caller makes the context current and holds the wgpu GL state (see
//! `graphics::surface::WgpuGlContext`) around the call.

use egui::epaint::{ClippedPrimitive, textures::TexturesDelta};

/// The colour each frame is cleared to before egui draws: transparent, or
/// opaque red when `COMPD_UI_DEBUG_CLEAR` is set (read once). A diagnosis
/// switch: a surface that shows red has its clear reach the texture the
/// compositor samples while egui's meshes do not; one that stays black never
/// reaches that texture at all.
fn clear_colour() -> wgpu::Color {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DEBUG.get_or_init(|| std::env::var_os("COMPD_UI_DEBUG_CLEAR").is_some()) {
        wgpu::Color::RED
    } else {
        wgpu::Color::TRANSPARENT
    }
}

pub struct Painter {
    renderer: egui_wgpu::Renderer,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Painter").finish_non_exhaustive()
    }
}

impl Painter {
    /// A painter for targets of `format` (`graphics::surface::FORMAT`).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                // Exact colours: the compositor samples these pixels as they are.
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        Self { renderer }
    }

    /// Upload `textures`, paint `primitives` over a cleared target, submit,
    /// then free the textures the pass retired. `size_px` is the target's
    /// size in physical pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size_px: [u32; 2],
        pixels_per_point: f32,
        primitives: &[ClippedPrimitive],
        mut textures: TexturesDelta,
    ) {
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("compd_ui_egui"),
        });
        let mut buffers =
            self.renderer
                .update_buffers(device, queue, &mut encoder, primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("compd_ui_egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_colour()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, primitives, &screen);
        }
        buffers.push(encoder.finish());
        queue.submit(buffers);
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        // Applied: epaint asserts a delta is never dropped unapplied.
        textures.clear();
    }
}
