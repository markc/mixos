// SPDX-License-Identifier: MIT OR Apache-2.0
//! The GPU side of layer surfaces: one wgpu instance on the connection's
//! display and one device for every surface (made with the first), and per
//! surface a wgpu surface on its `wl_surface` and an egui-wgpu renderer
//! (egui contexts number their textures independently, so a renderer is
//! never shared between them).

use std::ffi::c_void;
use std::ptr::NonNull;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle,
};

/// libwayland's display, handed to wgpu so its GL backend opens EGL on it.
#[derive(Debug)]
struct Display(NonNull<c_void>);

// SAFETY: a `wl_display` pointer is only an address here; libwayland's
// client side is thread-safe, and the connection outlives the instance.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl HasDisplayHandle for Display {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.0));
        // SAFETY: the pointer is the live connection's display.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

struct Device {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// The instance, and the device once the first surface made it.
pub(crate) struct Gpu {
    display: NonNull<c_void>,
    instance: wgpu::Instance,
    device: Option<Device>,
}

/// One surface's swapchain and renderer.
pub(crate) struct Target {
    // Dropped before the device and the `wl_surface` it draws on.
    surface: wgpu::Surface<'static>,
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
    alpha: wgpu::CompositeAlphaMode,
    configured: Option<(u32, u32)>,
}

/// The frame was not drawn; what to do about it.
pub(crate) enum Skipped {
    /// The swapchain is stale: reconfigure, then draw again.
    Reconfigure,
    /// Not now (timeout, occluded): draw again on the next change.
    Later,
}

impl Gpu {
    /// A GL instance on `display` (the connection's `wl_display`).
    pub(crate) fn new(display: *mut c_void) -> Result<Gpu, String> {
        let display = NonNull::new(display).ok_or("no wl_display pointer")?;
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::GL;
        desc.display = Some(Box::new(Display(display)));
        Ok(Gpu {
            display,
            instance: wgpu::Instance::new(desc),
            device: None,
        })
    }

    /// The limit egui's font atlas and images must fit.
    pub(crate) fn max_texture_side(&self) -> Option<usize> {
        self.device
            .as_ref()
            .map(|d| d.device.limits().max_texture_dimension_2d as usize)
    }

    /// A target on `surface` (a live `wl_surface` pointer). The first one
    /// also makes the device, from an adapter that can present to it.
    pub(crate) fn target(&mut self, surface: *mut c_void) -> Result<Target, String> {
        let window = NonNull::new(surface).ok_or("no wl_surface pointer")?;
        // SAFETY: the display and the surface belong to the live connection
        // and layer surface, which outlive the target (it is dropped first).
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(RawDisplayHandle::Wayland(
                        WaylandDisplayHandle::new(self.display),
                    )),
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                })
        }
        .map_err(|e| format!("wgpu surface: {e}"))?;
        if self.device.is_none() {
            let adapter = futures::executor::block_on(self.instance.request_adapter(
                &wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                },
            ))
            .map_err(|e| format!("adapter: {e}"))?;
            let (device, queue) = futures::executor::block_on(
                adapter.request_device(&wgpu::DeviceDescriptor::default()),
            )
            .map_err(|e| format!("device: {e}"))?;
            self.device = Some(Device {
                adapter,
                device,
                queue,
            });
        }
        let d = self.device.as_ref().ok_or("no device")?;
        let caps = surface.get_capabilities(&d.adapter);
        // egui-wgpu writes non-sRGB targets.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("the surface offers no format")?;
        let alpha = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let renderer = egui_wgpu::Renderer::new(
            &d.device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        Ok(Target {
            surface,
            renderer,
            format,
            alpha,
            configured: None,
        })
    }

    /// Size `target`'s swapchain to `size_px` (a no-op at the same size).
    pub(crate) fn configure(&self, target: &mut Target, size_px: (u32, u32)) {
        let Some(d) = &self.device else { return };
        if size_px.0 == 0 || size_px.1 == 0 || target.configured == Some(size_px) {
            return;
        }
        target.surface.configure(
            &d.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: target.format,
                width: size_px.0,
                height: size_px.1,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: target.alpha,
                color_space: wgpu::SurfaceColorSpace::Auto,
                view_formats: Vec::new(),
            },
        );
        target.configured = Some(size_px);
    }

    /// Paint one egui pass into `target` and present it. `before_present`
    /// runs between the submit and the present (ask for a frame callback
    /// there: the present commits the surface).
    pub(crate) fn paint(
        &self,
        target: &mut Target,
        output: egui::FullOutput,
        ctx: &egui::Context,
        before_present: impl FnOnce(),
    ) -> Result<(), Skipped> {
        let Some(d) = &self.device else {
            return Err(Skipped::Later);
        };
        let Some(size_px) = target.configured else {
            return Err(Skipped::Reconfigure);
        };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                target
                    .renderer
                    .update_texture(&d.device, &d.queue, *id, delta);
            }
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let frame = match target.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                target.configured = None;
                return Err(Skipped::Reconfigure);
            }
            _ => return Err(Skipped::Later),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size_px.0, size_px.1],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = d
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("toolkit_layer"),
            });
        let mut buffers =
            target
                .renderer
                .update_buffers(&d.device, &d.queue, &mut encoder, &primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("toolkit_layer_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            target.renderer.render(&mut pass, &primitives, &screen);
        }
        buffers.push(encoder.finish());
        d.queue.submit(buffers);
        for id in &output.textures_delta.free {
            target.renderer.free_texture(id);
        }
        before_present();
        d.queue.present(frame);
        Ok(())
    }
}
