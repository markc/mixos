// SPDX-License-Identifier: MIT OR Apache-2.0
//! `UiRuntime<P>`: one egui UI instance, with no window system.
//!
//! Input arrives as `egui::Event`s the compositor translated; frames go into a
//! caller-supplied `wgpu::TextureView`. The lifecycle the registry drives:
//!
//! 1. `queue_event` / `queue_message` as input arrives;
//! 2. `tick` runs an egui pass when something wants one (input, messages,
//!    a due repaint request, a resize) and reports whether pixels changed;
//! 3. `render_into` paints the latest pass (running one first if a pass is
//!    still owed), or `acknowledge_frame` when the surface is off screen.
//!
//! A pass's texture uploads are kept until a paint consumes them, so passes
//! that are never painted (an off-screen surface) lose no font-atlas or image
//! updates.

use std::time::{Duration, Instant};

use egui::epaint::{ClippedShape, textures::TexturesDelta};
use egui::{Context, Event, Pos2, RawInput, Rect, Vec2, ViewportId};

use super::painter::Painter;
use super::program::UiProgram;
use super::wake::RuntimeWake;

/// Observes every message a runtime applies, before `update` runs. The
/// compositor uses it to act on what the user did without owning the
/// program's state.
pub trait MessageHandler<M>: Send + 'static {
    fn handle(&mut self, message: &M);
}

impl<M, F> MessageHandler<M> for F
where
    F: FnMut(&M) + Send + 'static,
{
    fn handle(&mut self, message: &M) {
        (self)(message)
    }
}

fn dispatch<P: UiProgram>(
    program: &mut P,
    handler: &mut Option<Box<dyn MessageHandler<P::Message>>>,
    messages: Vec<P::Message>,
) -> bool {
    let applied = !messages.is_empty();
    let mut queue: std::collections::VecDeque<_> = messages.into();
    while let Some(message) = queue.pop_front() {
        if let Some(handler) = handler.as_mut() {
            handler.handle(&message);
        }
        program.update(message.clone());
        queue.extend(program.process(&message));
    }
    applied
}

/// The shapes of the latest pass, kept so a surface that was released and
/// re-allocated can be repainted without another pass.
struct Pass {
    shapes: Vec<ClippedShape>,
    pixels_per_point: f32,
}

/// One egui UI instance.
pub struct UiRuntime<P: UiProgram> {
    /// The program's state. Public for direct reads and for updates that
    /// bypass the message queue.
    pub program: P,
    ctx: Context,
    size_px: (u32, u32),
    scale: f32,
    events: Vec<Event>,
    messages: Vec<P::Message>,
    focused: bool,
    handler: Option<Box<dyn MessageHandler<P::Message>>>,
    /// A pass is owed: new instance, resize, applied messages or input.
    needs_pass: bool,
    /// Pixels changed since the last paint (or acknowledgement).
    dirty: bool,
    last: Option<Pass>,
    /// Texture uploads and frees not yet painted, oldest first.
    textures: TexturesDelta,
    painter: Option<Painter>,
    max_texture_side: Option<usize>,
    wake: RuntimeWake,
    started: Instant,
    set_up: bool,
}

impl<P: UiProgram> Drop for UiRuntime<P> {
    fn drop(&mut self) {
        // Uploads a surface never painted go with it; epaint asserts a delta
        // is not dropped unapplied.
        self.textures.clear();
    }
}

impl<P: UiProgram> std::fmt::Debug for UiRuntime<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiRuntime")
            .field("size_px", &self.size_px)
            .field("scale", &self.scale)
            .field("events", &self.events.len())
            .field("messages", &self.messages.len())
            .field("needs_pass", &self.needs_pass)
            .field("dirty", &self.dirty)
            .finish()
    }
}

impl<P: UiProgram> UiRuntime<P> {
    /// A runtime for `program` at `size_px` physical pixels. `scale` is
    /// physical pixels per egui point.
    pub fn new(program: P, size_px: (u32, u32), scale: f32) -> Self {
        Self::with_wake(program, size_px, scale, RuntimeWake::new())
    }

    pub(crate) fn with_wake(
        program: P,
        size_px: (u32, u32),
        scale: f32,
        wake: RuntimeWake,
    ) -> Self {
        let ctx = Context::default();
        let remote = wake.clone();
        ctx.set_request_repaint_callback(move |info| remote.request(info.delay));
        Self {
            program,
            ctx,
            size_px: (size_px.0.max(1), size_px.1.max(1)),
            scale: if scale > 0.0 { scale } else { 1.0 },
            events: Vec::new(),
            messages: Vec::new(),
            focused: true,
            handler: None,
            // The first frame draws even if no input ever arrives.
            needs_pass: true,
            dirty: true,
            last: None,
            textures: TexturesDelta::default(),
            painter: None,
            max_texture_side: None,
            wake,
            started: Instant::now(),
            set_up: false,
        }
    }

    /// The egui context, for read-back (widget rects, memory) between passes.
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Install a message handler, replacing any earlier one.
    pub fn set_message_handler<H: MessageHandler<P::Message>>(&mut self, handler: H) {
        self.handler = Some(Box::new(handler));
    }

    pub fn clear_message_handler(&mut self) {
        self.handler = None;
    }

    // ── Ingest ──────────────────────────────────────────────────────────

    /// Queue a translated input event for the next pass.
    pub fn queue_event(&mut self, event: Event) {
        if let Event::WindowFocused(focused) = &event {
            self.focused = *focused;
        }
        self.events.push(event);
        self.needs_pass = true;
    }

    /// The pointer left the surface.
    pub fn pointer_leave(&mut self) {
        self.queue_event(Event::PointerGone);
    }

    /// Queue a message for the next pass (applied before it draws).
    pub fn queue_message(&mut self, message: P::Message) {
        self.messages.push(message);
        self.needs_pass = true;
    }

    /// Input, messages or a repaint request wait for the next pass. Only a
    /// frame ticks the runtime, so the host owes one while this is true.
    pub fn has_queued(&self) -> bool {
        self.wake.pending() || !self.events.is_empty() || !self.messages.is_empty()
    }

    // ── Tick ────────────────────────────────────────────────────────────

    /// Run a pass if one is owed. Returns whether a paint is wanted.
    pub fn tick(&mut self) -> bool {
        let woke = self.wake.take();
        if woke || self.needs_pass {
            self.run_pass();
        }
        self.dirty
    }

    fn raw_input(&mut self) -> RawInput {
        let size = Vec2::new(
            self.size_px.0 as f32 / self.scale,
            self.size_px.1 as f32 / self.scale,
        );
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            max_texture_side: self.max_texture_side,
            time: Some(self.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..RawInput::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.scale);
        raw
    }

    fn run_pass(&mut self) {
        if !self.set_up {
            self.program.setup(&self.ctx);
            self.set_up = true;
        }
        // Messages queued from outside apply before the pass draws them.
        let queued = std::mem::take(&mut self.messages);
        dispatch(&mut self.program, &mut self.handler, queued);

        let raw = self.raw_input();
        let ctx = self.ctx.clone();
        let mut emitted = Vec::new();
        let program = &mut self.program;
        let output = ctx.run_ui(raw, |ui| program.show(ui, &mut emitted));
        self.textures.append(output.textures_delta);
        self.last = Some(Pass {
            shapes: output.shapes,
            pixels_per_point: output.pixels_per_point,
        });
        self.dirty = true;
        // What the pass emitted changed state the pass has not drawn yet.
        self.needs_pass = dispatch(&mut self.program, &mut self.handler, emitted);
        if self.needs_pass {
            self.wake.request(Duration::ZERO);
        }
    }

    // ── Render ──────────────────────────────────────────────────────────

    /// Paint the latest pass into `view`, running a pass first if one is
    /// owed. `view`'s texture is `size_px` physical pixels in `format`
    /// (`graphics::surface::FORMAT`). The GL context must be current with
    /// the wgpu GL state acquired.
    pub fn render_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        view: &wgpu::TextureView,
    ) {
        if self.needs_pass || self.last.is_none() || self.wake.take() {
            self.run_pass();
        }
        let painter = self
            .painter
            .get_or_insert_with(|| Painter::new(device, format));
        self.max_texture_side = Some(device.limits().max_texture_dimension_2d as usize);
        let Some(pass) = &self.last else { return };
        let primitives = self
            .ctx
            .tessellate(pass.shapes.clone(), pass.pixels_per_point);
        let textures = std::mem::take(&mut self.textures);
        painter.paint(
            device,
            queue,
            view,
            [self.size_px.0, self.size_px.1],
            pass.pixels_per_point,
            &primitives,
            textures,
        );
        self.dirty = false;
    }

    // ── State ───────────────────────────────────────────────────────────

    /// A frame is wanted now: a pass is owed, pixels are unpainted, or a
    /// repaint request is due.
    pub fn is_dirty(&self) -> bool {
        self.needs_pass || self.dirty || self.wake.pending()
    }

    /// The future instant a delayed repaint request waits for.
    pub fn deadline(&self) -> Option<Instant> {
        self.wake.deadline()
    }

    /// Animating: a repaint is wanted now or later.
    pub fn is_animating(&self) -> bool {
        self.wake.pending() || self.wake.deadline().is_some()
    }

    /// Force a pass and paint on the next frame.
    pub fn request_redraw(&mut self) {
        self.needs_pass = true;
    }

    /// Acknowledge a frame without painting: an off-screen surface stops
    /// asking for frames. Its texture uploads wait for the next paint.
    pub fn acknowledge_frame(&mut self) {
        self.dirty = false;
    }

    /// Change the size (physical pixels) and scale (pixels per point).
    pub fn resize(&mut self, size_px: (u32, u32), scale: f32) {
        self.size_px = (size_px.0.max(1), size_px.1.max(1));
        if scale > 0.0 {
            self.scale = scale;
        }
        self.needs_pass = true;
    }

    pub fn physical_size(&self) -> (u32, u32) {
        self.size_px
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale
    }
}
