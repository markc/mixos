// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Wayland side of layer surfaces and the loop that drives them.
//!
//! One calloop loop holds the connection, a channel egui's repaint callbacks
//! send on (from any thread: `Context::request_repaint`), and one-shot
//! timers for delayed repaints. After each dispatch every surface that is
//! due and not waiting on a frame callback draws once.

use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{Channel, Event as ChannelEvent, Sender};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager;
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, registry_handlers,
};

use super::gpu::{Gpu, Skipped, Target};
use super::{LayerOptions, LayerProgram, Outputs, SurfaceInfo, input, scale};

/// A repaint asked for: which surface, and in how long.
type Repaint = (u64, Duration);

struct Surface {
    // Dropped first: the GPU target before the Wayland surface it draws on.
    target: Target,
    id: u64,
    output: Option<wl_output::WlOutput>,
    output_name: Option<String>,
    layer: LayerSurface,
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    ctx: egui::Context,
    events: Vec<egui::Event>,
    size: (u32, u32),
    scale: f32,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    /// The earliest delayed repaint a timer is armed for.
    deadline: Option<Instant>,
    focused: bool,
    started: Instant,
}

struct State<P> {
    gpu: Gpu,
    surfaces: Vec<Surface>,
    registry: RegistryState,
    seat: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    fractional: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    cursor_shapes: Option<CursorShapeManager>,
    pointer: Option<wl_pointer::WlPointer>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    /// The surface the pointer is over, and the serial of its enter.
    pointer_over: Option<(ObjectId, u32)>,
    cursor: egui::CursorIcon,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_focus: Option<ObjectId>,
    modifiers: Modifiers,
    program: P,
    options: LayerOptions,
    repaint: Sender<Repaint>,
    handle: LoopHandle<'static, State<P>>,
    next_id: u64,
    exit: bool,
}

pub(super) fn run<P: LayerProgram + 'static>(
    options: LayerOptions,
    program: P,
) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
    let (globals, queue) =
        registry_queue_init::<State<P>>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, State<P>> =
        EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|e| format!("wayland source: {e}"))?;
    let (repaint, requests): (Sender<Repaint>, Channel<Repaint>) =
        smithay_client_toolkit::reexports::calloop::channel::channel();
    handle
        .insert_source(requests, |event, _, state: &mut State<P>| {
            if let ChannelEvent::Msg((id, delay)) = event {
                state.request(id, delay);
            }
        })
        .map_err(|e| format!("repaint channel: {e}"))?;

    let gpu = Gpu::new(conn.backend().display_ptr().cast())?;
    let mut state = State {
        gpu,
        surfaces: Vec::new(),
        registry: RegistryState::new(&globals),
        seat: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh)
            .map_err(|e| format!("wl_compositor: {e}"))?,
        shell: LayerShell::bind(&globals, &qh).map_err(|e| format!("layer shell: {e}"))?,
        fractional: globals.bind(&qh, 1..=1, GlobalData).ok(),
        viewporter: globals.bind(&qh, 1..=1, GlobalData).ok(),
        cursor_shapes: CursorShapeManager::bind(&globals, &qh).ok(),
        pointer: None,
        shape_device: None,
        pointer_over: None,
        cursor: egui::CursorIcon::Default,
        keyboard: None,
        keyboard_focus: None,
        modifiers: Modifiers::default(),
        program,
        options,
        repaint,
        handle,
        next_id: 0,
        exit: false,
    };
    if state.options.outputs == Outputs::Compositor {
        state.add_surface(&qh, None)?;
    }
    while !state.exit {
        event_loop
            .dispatch(None, &mut state)
            .map_err(|e| format!("dispatch: {e}"))?;
        state.draw_due(&qh);
    }
    Ok(())
}

impl<P: LayerProgram + 'static> State<P> {
    fn add_surface(
        &mut self,
        qh: &QueueHandle<Self>,
        output: Option<wl_output::WlOutput>,
    ) -> Result<(), String> {
        let wl = self.compositor.create_surface(qh);
        let layer = self.shell.create_layer_surface(
            qh,
            wl,
            self.options.layer,
            Some(self.options.namespace.clone()),
            output.as_ref(),
        );
        layer.set_anchor(self.options.anchor);
        layer.set_exclusive_zone(self.options.exclusive_zone);
        layer.set_keyboard_interactivity(self.options.keyboard);
        layer.set_size(self.options.size.0, self.options.size.1);
        let id = self.next_id;
        self.next_id += 1;
        let fractional = self
            .fractional
            .as_ref()
            .map(|m| m.get_fractional_scale(layer.wl_surface(), qh, id));
        let viewport = match (&fractional, &self.viewporter) {
            (Some(_), Some(v)) => Some(v.get_viewport(layer.wl_surface(), qh, GlobalData)),
            _ => None,
        };
        layer.commit();
        let target = self.gpu.target(layer.wl_surface().id().as_ptr().cast())?;
        let ctx = egui::Context::default();
        let sender = self.repaint.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = sender.send((id, info.delay));
        });
        self.program.setup(&ctx);
        let output_name = output
            .as_ref()
            .and_then(|o| self.outputs.info(o))
            .and_then(|i| i.name);
        self.surfaces.push(Surface {
            target,
            id,
            output,
            output_name,
            layer,
            viewport,
            fractional,
            ctx,
            events: Vec::new(),
            size: (0, 0),
            scale: 1.0,
            configured: false,
            dirty: true,
            frame_pending: false,
            deadline: None,
            focused: false,
            started: Instant::now(),
        });
        Ok(())
    }

    fn surface_mut(&mut self, id: u64) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|s| s.id == id)
    }

    fn by_wl(&mut self, wl: &ObjectId) -> Option<&mut Surface> {
        self.surfaces
            .iter_mut()
            .find(|s| s.layer.wl_surface().id() == *wl)
    }

    /// A repaint egui asked for: now, or by a one-shot timer.
    fn request(&mut self, id: u64, delay: Duration) {
        let handle = self.handle.clone();
        let Some(s) = self.surface_mut(id) else {
            return;
        };
        if delay.is_zero() {
            s.dirty = true;
            return;
        }
        // egui uses Duration::MAX for "not until something happens".
        let Some(at) = Instant::now().checked_add(delay) else {
            return;
        };
        if s.deadline.is_some_and(|d| d <= at) {
            return;
        }
        s.deadline = Some(at);
        let armed = handle.insert_source(Timer::from_deadline(at), move |_, _, state| {
            if let Some(s) = state.surface_mut(id)
                && s.deadline.is_some_and(|d| d <= Instant::now())
            {
                s.deadline = None;
                s.dirty = true;
            }
            TimeoutAction::Drop
        });
        if armed.is_err()
            && let Some(s) = self.surface_mut(id)
        {
            // No timer: draw now rather than never.
            s.deadline = None;
            s.dirty = true;
        }
    }

    /// Size and scale changed: resize the swapchain and the viewport.
    fn reconfigure(&mut self, id: u64) {
        let Some(at) = self.surfaces.iter().position(|s| s.id == id) else {
            return;
        };
        let s = &mut self.surfaces[at];
        let px = scale::buffer_size(s.size, s.scale);
        if let Some(viewport) = &s.viewport {
            viewport.set_destination(s.size.0 as i32, s.size.1 as i32);
        }
        self.gpu.configure(&mut s.target, px);
        s.dirty = true;
    }

    fn draw_due(&mut self, qh: &QueueHandle<Self>) {
        let due: Vec<u64> = self
            .surfaces
            .iter()
            .filter(|s| s.configured && s.dirty && !s.frame_pending)
            .map(|s| s.id)
            .collect();
        for id in due {
            self.draw(qh, id);
        }
    }

    /// One egui pass on one surface, painted and presented.
    fn draw(&mut self, qh: &QueueHandle<Self>, id: u64) {
        let max_texture_side = self.gpu.max_texture_side();
        let Some(at) = self.surfaces.iter().position(|s| s.id == id) else {
            return;
        };
        let s = &mut self.surfaces[at];
        s.dirty = false;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(s.size.0 as f32, s.size.1 as f32),
            )),
            max_texture_side,
            time: Some(s.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut s.events),
            focused: s.focused,
            ..egui::RawInput::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(s.scale);
        let info = SurfaceInfo {
            output: s.output_name.clone(),
            width: s.size.0,
            height: s.size.1,
            scale: s.scale,
        };
        let program = &mut self.program;
        let output = s.ctx.run_ui(raw, |ui| program.show(ui, &info));
        let cursor = output.platform_output.cursor_icon;
        let wl = s.layer.wl_surface().clone();
        let painted = self.gpu.paint(&mut s.target, output, &s.ctx, || {
            // The present commits the surface: the frame callback goes in
            // with it, so the next pass waits for this one to be shown.
            wl.frame(qh, wl.clone());
        });
        match painted {
            Ok(()) => s.frame_pending = true,
            Err(Skipped::Reconfigure) => self.reconfigure(id),
            Err(Skipped::Later) => s.dirty = true,
        }
        let over = self
            .pointer_over
            .as_ref()
            .is_some_and(|(over, _)| *over == wl.id());
        if over && cursor != self.cursor {
            self.cursor = cursor;
            self.set_cursor();
        }
    }

    fn set_cursor(&self) {
        if let (Some(device), Some((_, serial))) = (&self.shape_device, &self.pointer_over) {
            device.set_shape(*serial, input::shape(self.cursor));
        }
    }

    fn push_to(&mut self, wl: &ObjectId, event: egui::Event) {
        if let Some(s) = self.by_wl(wl) {
            s.events.push(event);
            s.dirty = true;
        }
    }

    fn key(&mut self, event: &KeyEvent, pressed: bool, repeat: bool) {
        let Some(focus) = self.keyboard_focus.clone() else {
            return;
        };
        let modifiers = input::modifiers(&self.modifiers);
        let text = event.utf8.as_deref();
        if let Some(key) = input::key(event.keysym.raw(), text) {
            self.push_to(
                &focus,
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat,
                    modifiers,
                },
            );
        }
        if pressed && let Some(text) = input::typed(text, &self.modifiers) {
            self.push_to(&focus, egui::Event::Text(text));
        }
    }
}

impl<P: LayerProgram + 'static> CompositorHandler for State<P> {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // The integer scale, only when the fractional one is unavailable.
        let Some(s) = self.by_wl(&surface.id()) else {
            return;
        };
        if s.fractional.is_some() {
            return;
        }
        let factor = new_factor.max(1);
        if s.scale != factor as f32 {
            s.scale = factor as f32;
            surface.set_buffer_scale(factor);
            let id = s.id;
            self.reconfigure(id);
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

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(s) = self.by_wl(&surface.id()) {
            s.frame_pending = false;
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

impl<P: LayerProgram + 'static> OutputHandler for State<P> {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.options.outputs == Outputs::Each
            && let Err(error) = self.add_surface(qh, Some(output))
        {
            eprintln!("toolkit::layer: no surface for a new output: {error}");
        }
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let name = self.outputs.info(&output).and_then(|i| i.name);
        for s in &mut self.surfaces {
            if s.output.as_ref() == Some(&output) && s.output_name != name {
                s.output_name = name.clone();
                s.dirty = true;
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.surfaces.retain(|s| s.output.as_ref() != Some(&output));
    }
}

impl<P: LayerProgram + 'static> LayerShellHandler for State<P> {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        let wl = layer.wl_surface().id();
        self.surfaces.retain(|s| s.layer.wl_surface().id() != wl);
        if self.surfaces.is_empty() && self.options.outputs == Outputs::Compositor {
            self.exit = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let asked = self.options.size;
        let Some(s) = self.by_wl(&layer.wl_surface().id()) else {
            return;
        };
        let (mut width, mut height) = configure.new_size;
        if width == 0 {
            width = asked.0;
        }
        if height == 0 {
            height = asked.1;
        }
        if width == 0 || height == 0 {
            return;
        }
        s.configured = true;
        s.dirty = true;
        if (width, height) != s.size {
            s.size = (width, height);
            let id = s.id;
            self.reconfigure(id);
        }
    }
}

impl<P: LayerProgram + 'static> SeatHandler for State<P> {
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
            if let (Some(pointer), Some(shapes)) = (&self.pointer, &self.cursor_shapes) {
                self.shape_device = Some(shapes.get_shape_device(pointer, qh));
            }
        }
        if capability == Capability::Keyboard
            && self.keyboard.is_none()
            && self.options.keyboard != super::KeyboardInteractivity::None
        {
            self.keyboard = self
                .seat
                .get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.handle.clone(),
                    Box::new(|state, _, event| state.key(&event, true, true)),
                )
                .ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(device) = self.shape_device.take() {
                device.destroy();
            }
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl<P: LayerProgram + 'static> PointerHandler for State<P> {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let wl = event.surface.id();
            let pos = egui::pos2(event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.pointer_over = Some((wl.clone(), serial));
                    self.cursor = egui::CursorIcon::Default;
                    self.set_cursor();
                    self.push_to(&wl, egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Leave { .. } => {
                    if self.pointer_over.as_ref().is_some_and(|(o, _)| *o == wl) {
                        self.pointer_over = None;
                    }
                    self.push_to(&wl, egui::Event::PointerGone);
                }
                PointerEventKind::Motion { .. } => {
                    self.push_to(&wl, egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Press { button, .. }
                | PointerEventKind::Release { button, .. } => {
                    if let Some(button) = input::button(button) {
                        let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                        let modifiers = input::modifiers(&self.modifiers);
                        self.push_to(
                            &wl,
                            egui::Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                modifiers,
                            },
                        );
                    }
                }
                PointerEventKind::Axis {
                    ref horizontal,
                    ref vertical,
                    ..
                } => {
                    let modifiers = input::modifiers(&self.modifiers);
                    self.push_to(
                        &wl,
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: input::wheel(horizontal.absolute, vertical.absolute),
                            phase: egui::TouchPhase::Move,
                            modifiers,
                        },
                    );
                }
            }
        }
    }
}

impl<P: LayerProgram + 'static> KeyboardHandler for State<P> {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.id());
        if let Some(s) = self.by_wl(&surface.id()) {
            s.focused = true;
            s.events.push(egui::Event::WindowFocused(true));
            s.dirty = true;
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self.keyboard_focus.as_ref() == Some(&surface.id()) {
            self.keyboard_focus = None;
        }
        if let Some(s) = self.by_wl(&surface.id()) {
            s.focused = false;
            s.events.push(egui::Event::WindowFocused(false));
            s.dirty = true;
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event, true, false);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event, false, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: u32,
    ) {
        self.modifiers = modifiers;
        if let Some(focus) = self.keyboard_focus.clone() {
            let m = input::modifiers(&modifiers);
            self.push_to(&focus, egui::Event::ModifiersChanged(m));
        }
    }
}

impl<P: LayerProgram + 'static> ProvidesRegistryState for State<P> {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

// The fractional-scale and viewporter objects: only `preferred_scale` is an
// event, keyed by the surface id it was made for.
impl<P: LayerProgram + 'static> Dispatch<WpFractionalScaleManagerV1, GlobalData> for State<P> {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl<P: LayerProgram + 'static> Dispatch<WpFractionalScaleV1, u64> for State<P> {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale: value } = event
            && let Some(s) = state.surface_mut(*id)
        {
            let scale = scale::from_120ths(value);
            if s.scale != scale {
                s.scale = scale;
                state.reconfigure(*id);
            }
        }
    }
}

impl<P: LayerProgram + 'static> Dispatch<WpViewporter, GlobalData> for State<P> {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl<P: LayerProgram + 'static> Dispatch<WpViewport, GlobalData> for State<P> {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_compositor!(@<P: LayerProgram + 'static> State<P>);
delegate_output!(@<P: LayerProgram + 'static> State<P>);
delegate_layer!(@<P: LayerProgram + 'static> State<P>);
delegate_seat!(@<P: LayerProgram + 'static> State<P>);
delegate_pointer!(@<P: LayerProgram + 'static> State<P>);
delegate_keyboard!(@<P: LayerProgram + 'static> State<P>);
delegate_registry!(@<P: LayerProgram + 'static> State<P>);
