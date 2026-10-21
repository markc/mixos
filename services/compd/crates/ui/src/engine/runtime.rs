// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `UiRuntime<P>`: one egui UI instance, with no window system.
//!
//! Input arrives as [`crate::input::Input`] (or raw egui events); frames go
//! into a caller-supplied `wgpu::TextureView`. The lifecycle the registry
//! drives inside the scene's GPU phase (context current, wgpu GL state held):
//!
//! 1. `queue_input` / `queue_message` as input arrives;
//! 2. `tick` runs an egui pass when one is owed (input, messages, a due
//!    repaint request, a resize) and reports whether pixels changed;
//! 3. `render_into` paints the latest pass, or `acknowledge_frame` when the
//!    surface is off screen, with `set_suspended(true)` so its animations
//!    stop waking the loop until it is shown again.
//!
//! Outside that phase only `tick_offscreen` runs (compd parked, its VT in
//! the background): queued input and messages get their pass, the GPU is
//! not touched, and the pass's texture changes wait for the next pass or
//! paint inside a frame.
//!
//! A pass's texture changes are applied to the painter as the pass runs, not
//! when it is painted: the shapes kept for the next paint are always the
//! latest pass's, and they reference exactly the textures that leaves alive.
//! A runtime without a GPU (unit tests) merges them instead; a pass outside
//! a frame keeps its changes, whole and in order, for the next pass or
//! paint inside one.

use std::time::{Duration, Instant};

use egui::epaint::{ClippedShape, textures::TexturesDelta};
use egui::{Context, Event, Pos2, RawInput, Rect, Vec2, ViewportId};

use super::painter::Painter;
use super::program::UiProgram;
use super::wake::RuntimeWake;

/// Observes every message a runtime applies, before `update` runs. The
/// compositor uses it to act on what the user did without owning the
/// program's state. It may run outside a frame (`UiRuntime::tick_offscreen`,
/// compd parked): a handler sends or records, and never needs the renderer,
/// the seat or anything else only a frame provides.
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

/// The wgpu device a runtime paints with: the one sharing compd's GL context.
#[derive(Clone, Debug)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
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

/// Merge `newer` into `older` for a runtime with no painter. A texture
/// `newer` frees is gone at the end of its pass, so neither its earlier
/// uploads nor `newer`'s own updates to it may survive: a partial update
/// left behind would target a texture that was never allocated.
fn merge(older: &mut TexturesDelta, mut newer: TexturesDelta) {
    for id in &newer.free {
        older.set.remove(id);
    }
    let freed = newer.free.clone();
    for (id, deltas) in newer.set.drain() {
        if freed.contains(&id) {
            continue;
        }
        for delta in deltas {
            older.push(id, delta);
        }
    }
    older.free.extend(std::mem::take(&mut newer.free));
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
    translator: crate::input::Translator,
    messages: Vec<P::Message>,
    focused: bool,
    handler: Option<Box<dyn MessageHandler<P::Message>>>,
    /// A pass is owed: new instance, resize, applied messages or input.
    needs_pass: bool,
    /// Pixels changed since the last paint (or acknowledgement).
    dirty: bool,
    last: Option<Pass>,
    gpu: Option<Gpu>,
    painter: Option<Painter>,
    /// Texture changes of a runtime with no GPU (tests), merged.
    unapplied: TexturesDelta,
    /// Each pass's texture changes from passes run outside a frame
    /// (`tick_offscreen`), kept whole and in order, so the next pass or
    /// paint inside a frame applies them one by one exactly as if each pass
    /// had run there: a texture a pass uploads and frees (frees come after
    /// painting) is still there for that pass's paint.
    held: Vec<TexturesDelta>,
    /// A pass is running outside a frame: its textures wait in `held`.
    offscreen: bool,
    wake: RuntimeWake,
    started: Instant,
    set_up: bool,
}

impl<P: UiProgram> Drop for UiRuntime<P> {
    fn drop(&mut self) {
        // epaint asserts a delta is not dropped unapplied.
        self.unapplied.clear();
        for delta in &mut self.held {
            delta.clear();
        }
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
    /// A runtime for `program` at `size_px` physical pixels, `scale`
    /// physical pixels per egui point, painting with `gpu`.
    pub fn new(program: P, size_px: (u32, u32), scale: f32, gpu: Gpu) -> Self {
        Self::build(program, size_px, scale, Some(gpu), RuntimeWake::new())
    }

    /// A runtime with no GPU: passes run, nothing is painted (tests).
    pub fn headless(program: P, size_px: (u32, u32), scale: f32) -> Self {
        Self::build(program, size_px, scale, None, RuntimeWake::new())
    }

    pub(crate) fn build(
        program: P,
        size_px: (u32, u32),
        scale: f32,
        gpu: Option<Gpu>,
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
            translator: crate::input::Translator::default(),
            messages: Vec::new(),
            focused: true,
            handler: None,
            // The first frame draws even if no input ever arrives.
            needs_pass: true,
            dirty: true,
            last: None,
            gpu,
            painter: None,
            unapplied: TexturesDelta::default(),
            held: Vec::new(),
            offscreen: false,
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

    /// Queue compositor input for the next pass.
    pub fn queue_input(&mut self, input: crate::input::Input) {
        let before = self.events.len();
        self.translator.translate(input, &mut self.events);
        if self.events[before..]
            .iter()
            .any(|event| matches!(event, Event::WindowFocused(_)))
        {
            self.focused = self
                .events
                .iter()
                .rev()
                .find_map(|event| match event {
                    Event::WindowFocused(focused) => Some(*focused),
                    _ => None,
                })
                .unwrap_or(self.focused);
        }
        if self.events.len() > before {
            self.needs_pass = true;
        }
    }

    /// Queue an egui event as it is, for the next pass.
    pub fn queue_event(&mut self, event: Event) {
        if let Event::WindowFocused(focused) = &event {
            self.focused = *focused;
        }
        self.events.push(event);
        self.needs_pass = true;
    }

    /// The pointer left the surface.
    pub fn pointer_leave(&mut self) {
        self.queue_input(crate::input::Input::PointerLeft);
    }

    /// Queue a message for the next pass (applied before it draws).
    pub fn queue_message(&mut self, message: P::Message) {
        self.messages.push(message);
        self.needs_pass = true;
    }

    /// Input, messages or a repaint request wait for the next pass. Only a
    /// frame ticks the runtime, so the host owes one while this is true.
    pub fn has_queued(&self) -> bool {
        self.wake.pending() || self.has_input()
    }

    /// Input or messages (not a repaint request) wait for the next pass.
    pub fn has_input(&self) -> bool {
        !self.events.is_empty() || !self.messages.is_empty()
    }

    /// Run the pass queued input or messages owe, outside a frame (compd
    /// parked): the program sees the input and its messages dispatch, but
    /// the GPU is not touched. The pass's texture changes wait, in order, for
    /// the next pass that runs inside a frame (`run_pass`), where the
    /// compositor's GL context is current. A repaint request alone runs
    /// nothing here.
    pub fn tick_offscreen(&mut self) {
        if !self.has_input() {
            return;
        }
        self.offscreen = true;
        self.run_pass();
        self.offscreen = false;
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
        let max_texture_side = self
            .gpu
            .as_ref()
            .map(|gpu| gpu.device.limits().max_texture_dimension_2d as usize);
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            max_texture_side,
            time: Some(self.started.elapsed().as_secs_f64()),
            events: self.take_events(),
            focused: self.focused,
            ..RawInput::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.scale);
        raw
    }

    /// Apply the texture changes passes outside a frame held back, in
    /// order, before anything newer. Only inside a frame, and only with a
    /// GPU (a headless runtime keeps them for its tests).
    fn apply_held(&mut self) {
        if self.offscreen || self.gpu.is_none() {
            return;
        }
        for delta in std::mem::take(&mut self.held) {
            self.apply_textures(delta);
        }
    }

    fn run_pass(&mut self) {
        self.apply_held();
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
        self.apply_textures(output.textures_delta);
        self.last = Some(Pass {
            shapes: output.shapes,
            pixels_per_point: output.pixels_per_point,
        });
        self.dirty = true;
        // What the pass emitted changed state the pass has not drawn yet.
        self.needs_pass = dispatch(&mut self.program, &mut self.handler, emitted);
        // Input held back for the next pass (see `take_events`) runs now.
        self.needs_pass |= !self.events.is_empty();
        if self.needs_pass {
            self.wake.request(Duration::ZERO);
        }
    }

    /// The events for this pass. egui sees a button pressed and released in
    /// one pass as no click (a fast touchpad tap, or an injected click), so
    /// a pass ends at a press whose release is already queued and the
    /// release goes to the next pass.
    fn take_events(&mut self) -> Vec<Event> {
        let split = self.events.iter().enumerate().find_map(|(at, event)| match event {
            Event::PointerButton { button, pressed: true, .. } => self.events[at + 1..]
                .iter()
                .any(|later| {
                    matches!(later, Event::PointerButton { button: b, pressed: false, .. } if b == button)
                })
                .then_some(at),
            _ => None,
        });
        match split {
            Some(at) => {
                let rest = self.events.split_off(at + 1);
                std::mem::replace(&mut self.events, rest)
            }
            None => std::mem::take(&mut self.events),
        }
    }

    fn apply_textures(&mut self, textures: TexturesDelta) {
        match &self.gpu {
            Some(_) if self.offscreen => self.held.push(textures),
            Some(gpu) => {
                let painter = self
                    .painter
                    .get_or_insert_with(|| Painter::new(&gpu.device, gpu.format));
                painter.apply(&gpu.device, &gpu.queue, textures);
            }
            None => merge(&mut self.unapplied, textures),
        }
    }

    // ── Render ──────────────────────────────────────────────────────────

    /// Paint the latest pass into `view` (a texture of `size_px` physical
    /// pixels in the GPU's format), running a pass first if one is owed. A
    /// runtime with no GPU paints nothing.
    pub fn render_into(&mut self, view: &wgpu::TextureView) {
        self.set_suspended(false);
        if self.needs_pass || self.last.is_none() || self.wake.take() {
            self.run_pass();
        }
        // The latest pass may have run outside a frame.
        self.apply_held();
        let (Some(gpu), Some(pass)) = (&self.gpu, &self.last) else {
            return;
        };
        let painter = self
            .painter
            .get_or_insert_with(|| Painter::new(&gpu.device, gpu.format));
        let primitives = self
            .ctx
            .tessellate(pass.shapes.clone(), pass.pixels_per_point);
        painter.paint(
            &gpu.device,
            &gpu.queue,
            view,
            [self.size_px.0, self.size_px.1],
            pass.pixels_per_point,
            &primitives,
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
    /// asking for frames.
    pub fn acknowledge_frame(&mut self) {
        self.dirty = false;
    }

    /// Suspend (off screen) or resume. Suspended, repaint requests do not wake
    /// the loop or count as pending, so an animation nobody can see costs
    /// nothing; input and messages still run passes. Resuming owes a pass if
    /// any request arrived meanwhile.
    pub fn set_suspended(&mut self, suspended: bool) {
        if self.wake.set_suspended(suspended) {
            self.needs_pass = true;
        }
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

#[cfg(test)]
mod merge_tests {
    use super::merge;
    use egui::epaint::textures::TexturesDelta;
    use egui::epaint::{ColorImage, ImageDelta, TextureId, textures::TextureOptions};

    fn whole() -> ImageDelta {
        ImageDelta::full(
            ColorImage::filled([4, 4], egui::Color32::RED),
            TextureOptions::LINEAR,
        )
    }

    fn partial() -> ImageDelta {
        ImageDelta::partial(
            [1, 1],
            ColorImage::filled([2, 2], egui::Color32::BLUE),
            TextureOptions::LINEAR,
        )
    }

    #[test]
    fn a_texture_freed_later_takes_its_pending_updates_with_it() {
        let id = TextureId::Managed(7);
        let mut pending = TexturesDelta::default();
        let mut a = TexturesDelta::default();
        a.push(id, whole());
        merge(&mut pending, a);
        // Pass B updates part of it, then drops it.
        let mut b = TexturesDelta::default();
        b.push(id, partial());
        b.free(id);
        merge(&mut pending, b);
        assert!(
            !pending.set.contains_key(&id),
            "no partial update aimed at a freed texture"
        );
        assert!(pending.free.contains(&id));
        pending.clear();
    }

    #[test]
    fn a_partial_update_of_a_live_texture_survives() {
        let id = TextureId::Managed(3);
        let mut pending = TexturesDelta::default();
        let mut a = TexturesDelta::default();
        a.push(id, whole());
        merge(&mut pending, a);
        let mut b = TexturesDelta::default();
        b.push(id, partial());
        merge(&mut pending, b);
        assert_eq!(pending.set[&id].len(), 2);
        pending.clear();
    }
}
