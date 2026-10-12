// SPDX-License-Identifier: MIT OR Apache-2.0
//! One egui program on a wlr-layer-shell surface: the Wayland side
//! (smithay-client-toolkit), the GPU side (wgpu's GL backend on the
//! surface itself) and egui-wgpu painting between them.
//!
//! The surface sits on the Background layer, anchored to every edge with no
//! exclusive zone, so the compositor sizes it to the output. It draws only
//! when something changed (a configure, a scale change, input, or egui
//! asking for another pass), so an idle backdrop costs nothing.
//!
//! Kept apart from the program so it can move into `toolkit` once a second
//! program needs a layer surface.

use std::ffi::c_void;
use std::ptr::NonNull;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_output, wl_pointer, wl_seat, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, registry_handlers,
};

/// What a program sees of its surface each pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceInfo {
    /// Logical size, from the compositor's configure.
    pub width: u32,
    pub height: u32,
    /// The output's integer buffer scale.
    pub scale: i32,
}

/// An egui program drawn on the layer surface.
pub trait Program {
    /// Once, before the first pass.
    fn setup(&mut self, ctx: &egui::Context);
    /// One pass.
    fn show(&mut self, ui: &mut egui::Ui, surface: SurfaceInfo);
}

/// Where the surface goes and what the compositor is told about it.
#[derive(Clone, Debug)]
pub struct LayerOptions {
    pub layer: Layer,
    /// The namespace the compositor sees (its `layer` row names it).
    pub namespace: String,
    pub keyboard: KeyboardInteractivity,
}

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

struct Gpu {
    // Dropped first: the surface before the device and the Wayland surface.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: Option<wgpu::SurfaceConfiguration>,
    format: wgpu::TextureFormat,
    alpha: wgpu::CompositeAlphaMode,
    renderer: egui_wgpu::Renderer,
}

struct State<P> {
    gpu: Gpu,
    registry: RegistryState,
    seat: SeatState,
    outputs: OutputState,
    layer: LayerSurface,
    pointer: Option<wl_pointer::WlPointer>,
    ctx: egui::Context,
    program: P,
    events: Vec<egui::Event>,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    /// A pass is due.
    dirty: bool,
    /// A frame callback is pending (egui asked for another pass).
    frame_pending: bool,
    started: std::time::Instant,
    exit: bool,
}

/// Run `program` on a layer surface until the compositor closes it.
pub fn run<P: Program + 'static>(options: LayerOptions, program: P) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<State<P>>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let shell = LayerShell::bind(&globals, &qh).map_err(|e| format!("layer shell: {e}"))?;

    let surface = compositor.create_surface(&qh);
    let layer = shell.create_layer_surface(
        &qh,
        surface,
        options.layer,
        Some(options.namespace),
        None::<&wl_output::WlOutput>,
    );
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    // Under every exclusive zone, as a wallpaper is.
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(options.keyboard);
    layer.set_size(0, 0);
    layer.commit();

    let gpu = gpu(&conn, &layer)?;
    let ctx = egui::Context::default();
    let mut program = program;
    program.setup(&ctx);
    let mut state = State {
        gpu,
        registry: RegistryState::new(&globals),
        seat: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        layer,
        pointer: None,
        ctx,
        program,
        events: Vec::new(),
        size: (0, 0),
        scale: 1,
        configured: false,
        dirty: false,
        frame_pending: false,
        started: std::time::Instant::now(),
        exit: false,
    };
    while !state.exit {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("dispatch: {e}"))?;
        if state.dirty && state.configured && !state.frame_pending {
            state.render(&qh);
        }
    }
    Ok(())
}

/// The GPU side: a GL instance on the connection's display, a surface on
/// the layer's `wl_surface`, and a device that can present to it.
fn gpu(conn: &Connection, layer: &LayerSurface) -> Result<Gpu, String> {
    let display = NonNull::new(conn.backend().display_ptr().cast::<c_void>())
        .ok_or("no wl_display pointer")?;
    let window = NonNull::new(layer.wl_surface().id().as_ptr().cast::<c_void>())
        .ok_or("no wl_surface pointer")?;
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::GL;
    desc.display = Some(Box::new(Display(display)));
    let instance = wgpu::Instance::new(desc);
    // SAFETY: both handles belong to the live connection and layer surface,
    // which `run` keeps alive until after the GPU state is dropped.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                display,
            ))),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
        })
    }
    .map_err(|e| format!("wgpu surface: {e}"))?;
    let adapter =
        futures::executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;
    let (device, queue) =
        futures::executor::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("device: {e}"))?;
    let caps = surface.get_capabilities(&adapter);
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
        &device,
        format,
        egui_wgpu::RendererOptions {
            msaa_samples: 1,
            depth_stencil_format: None,
            dithering: false,
            predictable_texture_filtering: false,
        },
    );
    Ok(Gpu {
        surface,
        device,
        queue,
        config: None,
        format,
        alpha,
        renderer,
    })
}

impl<P: Program + 'static> State<P> {
    /// The surface's size in buffer pixels.
    fn size_px(&self) -> (u32, u32) {
        let scale = self.scale.max(1) as u32;
        (self.size.0 * scale, self.size.1 * scale)
    }

    fn reconfigure(&mut self) {
        let (width, height) = self.size_px();
        if width == 0 || height == 0 {
            return;
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.gpu.format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: self.gpu.alpha,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        };
        self.gpu.surface.configure(&self.gpu.device, &config);
        self.gpu.config = Some(config);
        self.dirty = true;
    }

    fn raw_input(&mut self) -> egui::RawInput {
        let scale = self.scale.max(1) as f32;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.size.0 as f32, self.size.1 as f32),
            )),
            max_texture_side: Some(self.gpu.device.limits().max_texture_dimension_2d as usize),
            time: Some(self.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..egui::RawInput::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        raw
    }

    /// One egui pass, painted and presented.
    fn render(&mut self, qh: &QueueHandle<Self>) {
        self.dirty = false;
        if self.gpu.config.is_none() {
            return;
        }
        let raw = self.raw_input();
        let info = SurfaceInfo {
            width: self.size.0,
            height: self.size.1,
            scale: self.scale,
        };
        let program = &mut self.program;
        let output = self.ctx.run_ui(raw, |ui| program.show(ui, info));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.gpu
                    .renderer
                    .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure();
                return;
            }
            _ => {
                // Timeout or occluded: try again on the next change.
                self.dirty = true;
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (width, height) = self.size_px();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("backdrop"),
            });
        let mut buffers = self.gpu.renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.gpu.renderer.render(&mut pass, &primitives, &screen);
        }
        buffers.push(encoder.finish());
        self.gpu.queue.submit(buffers);
        for id in &output.textures_delta.free {
            self.gpu.renderer.free_texture(id);
        }
        // Another pass wanted (an animation, a layout settling): ask for a
        // frame callback before this frame is committed, and draw then.
        let wants_more = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        if wants_more {
            let surface = self.layer.wl_surface();
            surface.frame(qh, surface.clone());
            self.frame_pending = true;
            self.dirty = true;
        }
        self.gpu.queue.present(frame);
    }
}

impl<P: Program + 'static> CompositorHandler for State<P> {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if new_factor != self.scale {
            self.scale = new_factor.max(1);
            surface.set_buffer_scale(self.scale);
            self.reconfigure();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty {
            self.render(qh);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl<P: Program + 'static> OutputHandler for State<P> {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl<P: Program + 'static> LayerShellHandler for State<P> {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            // Anchored to every edge, the compositor always picks the size.
            return;
        }
        self.configured = true;
        if (width, height) != self.size || self.gpu.config.is_none() {
            self.size = (width, height);
            self.reconfigure();
        }
        self.dirty = true;
    }
}

impl<P: Program + 'static> SeatHandler for State<P> {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

/// Linux input button codes (`BTN_LEFT` and on) as egui buttons.
fn egui_button(code: u32) -> Option<egui::PointerButton> {
    match code {
        0x110 => Some(egui::PointerButton::Primary),
        0x111 => Some(egui::PointerButton::Secondary),
        0x112 => Some(egui::PointerButton::Middle),
        _ => None,
    }
}

impl<P: Program + 'static> PointerHandler for State<P> {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if event.surface != *self.layer.wl_surface() {
                continue;
            }
            let pos = egui::pos2(event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.events.push(egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Leave { .. } => {
                    self.events.push(egui::Event::PointerGone);
                }
                PointerEventKind::Press { button, .. }
                | PointerEventKind::Release { button, .. } => {
                    if let Some(button) = egui_button(button) {
                        self.events.push(egui::Event::PointerButton {
                            pos,
                            button,
                            pressed: matches!(event.kind, PointerEventKind::Press { .. }),
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
                PointerEventKind::Axis { .. } => {}
            }
            self.dirty = true;
        }
    }
}

impl<P: Program + 'static> ProvidesRegistryState for State<P> {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(@<P: Program + 'static> State<P>);
delegate_output!(@<P: Program + 'static> State<P>);
delegate_layer!(@<P: Program + 'static> State<P>);
delegate_seat!(@<P: Program + 'static> State<P>);
delegate_pointer!(@<P: Program + 'static> State<P>);
delegate_registry!(@<P: Program + 'static> State<P>);
