//! Per-instance bundle: `UiSurface` + `UiRuntime<U>` + placement + space.
//!
//! `UiInstance<U>`: concrete, knows `U::Message`.
//! `UiItem`: type-erased newtype the registry stores and yields.
//! `UiInstanceAny`: private vtable behind `UiItem`.
//!
//! The item's `location` is in its own coordinate space (World coords for
//! `UiSpace::World`, screen physical pixels for `UiSpace::Screen`).
//! The registry and the compositor decide how to project this to screen
//! coords at render time using a camera `Transform` and output size,
//! passed as arguments. This mirrors how Smithay's `window.render_elements`
//! takes the screen location and zoom as arguments rather than storing them.

use std::any::{Any, TypeId};
use std::time::Instant;

use std::sync::Arc;

use crate::engine::{UiProgram, UiRuntime};
use crate::input::Input;
use graphics::bridge::publish::attempt::attempt::Attempt;
use graphics::surface::{SurfaceError, UiSurface, WgpuGlContext};
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Physical, Point, Rectangle, Size};

use crate::element::UiRenderElement;
use crate::handle::HandleId;
use crate::space::{Transform, UiSpace};

// ── Concrete instance ──────────────────────────────────────────────────

pub struct UiInstance<U: UiProgram> {
    pub(crate) id: HandleId,
    pub(crate) smithay_id: Id,
    pub(crate) commit: CommitCounter,
    pub(crate) location: Point<i32, Physical>,
    pub(crate) surface: UiSurface,
    pub(crate) scale_factor: f32,

    pub(crate) runtime: UiRuntime<U>,
    pub(crate) pending_resize: Option<(Size<i32, Physical>, f32)>,
    /// Last ring generation reflected in `commit`. Damage follows the ring, not
    /// the render call: a frame the ring did not publish is pixels the compositor
    /// already has, and reporting damage for it repaints the rect for nothing.
    pub(crate) generation: u64,
}

impl<U: UiProgram> UiInstance<U> {
    pub fn handle_id(&self) -> HandleId {
        self.id
    }
    pub fn location(&self) -> Point<i32, Physical> {
        self.location
    }
    pub fn size(&self) -> Size<i32, Physical> {
        self.surface.size
    }
    pub fn runtime(&self) -> &UiRuntime<U> {
        &self.runtime
    }
    pub fn runtime_mut(&mut self) -> &mut UiRuntime<U> {
        &mut self.runtime
    }
    pub fn ui(&self) -> &U {
        &self.runtime.program
    }
    pub fn ui_mut(&mut self) -> &mut U {
        &mut self.runtime.program
    }
}

// ── Vtable ─────────────────────────────────────────────────────────────

pub(crate) trait UiInstanceAny: Any {
    fn handle_id(&self) -> HandleId;
    fn smithay_id(&self) -> &Id;
    fn commit(&self) -> CommitCounter;
    fn bump_commit(&mut self);
    fn location(&self) -> Point<i32, Physical>;
    fn size(&self) -> Size<i32, Physical>;
    /// The UI viewport scale factor (logical = physical / scale_factor). For
    /// `World` items this is the zoom counter-scale (`1/zoom`); for `Screen`
    /// items it's the instance scale (currently always 1.0).
    fn scale_factor(&self) -> f32;
    fn set_location(&mut self, p: Point<i32, Physical>);
    fn queue_input(&mut self, input: Input);
    /// Events or messages wait for the next tick (`UiRuntime::has_queued`).
    fn has_queued(&self) -> bool;
    /// Input or messages wait (`UiRuntime::has_input`); a repaint request
    /// alone does not count.
    fn has_input(&self) -> bool;
    fn tick(&mut self) -> bool;
    /// Run what queued input owes outside a frame, GPU untouched
    /// (`UiRuntime::tick_offscreen`).
    fn tick_offscreen(&mut self);
    /// Advance frame bookkeeping WITHOUT rasterizing: clears the runtime's dirty
    /// flag so an off-screen surface stops pinning the redraw loop, while its
    /// animation/message driving (in `tick`) continues untouched. Paired with a
    /// `stale` flag on the item so it re-renders once when it becomes visible.
    fn acknowledge_frame(&mut self);
    /// Off screen (`true`) or shown again (see `UiRuntime::set_suspended`).
    fn set_suspended(&mut self, suspended: bool);
    /// True if the runtime still wants to be rendered next frame — dirty or
    /// mid-animation. Drives the host's "keep scheduling frames" decision.
    fn wants_frame(&self) -> bool;
    /// The future instant an animation asked to be redrawn at (UI
    /// `RedrawRequest::At`), which `wants_frame` deliberately does not cover.
    fn deadline(&self) -> Option<Instant>;
    fn render(&mut self);
    /// Retire a pipelined frame if the GPU finished it, WITHOUT rasterizing, and
    /// bump the commit when one becomes visible. Must run every frame for every
    /// item: UI rasterizes only when dirty, so a deferred publish has no render
    /// call of its own to ride along with and would otherwise never land.
    fn poll_publish(&mut self);
    /// Whether the GPU backing is currently allocated. Off-screen surfaces are
    /// released to reclaim memory while their runtime keeps ticking.
    fn is_resident(&self) -> bool;
    /// Free the GPU backing (the GL texture ring). The `UiRuntime` keeps
    /// running; call `ensure_backing` before the next render.
    fn release_backing(&mut self);
    /// Re-allocate the backing at the current size if it was released.
    fn ensure_backing(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> Result<(), SurfaceError>;
    /// Match the ring to the live setting. Per-frame, so the knob applies
    /// without a restart; a no-op while released or once the depth matches.
    fn sync_depth(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
        want: usize,
    ) -> Result<(), SurfaceError>;
    /// The GL texture UI last published, or `None` while the backing is
    /// released. The only output: UI draws into this texture on
    /// the compositor's own context.
    fn texture_handle(&self) -> Option<&smithay::backend::renderer::gles::GlesTexture>;
    fn apply_pending_resize(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> Result<bool, SurfaceError>;
    fn request_resize(&mut self, new_size: Size<i32, Physical>, scale_factor: f32);
    fn pending_resize(&self) -> Option<(Size<i32, Physical>, f32)>;
    fn pointer_leave(&mut self);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<U: UiProgram> UiInstanceAny for UiInstance<U> {
    fn handle_id(&self) -> HandleId {
        self.id
    }
    fn smithay_id(&self) -> &Id {
        &self.smithay_id
    }
    fn commit(&self) -> CommitCounter {
        self.commit
    }
    fn bump_commit(&mut self) {
        self.commit.increment();
    }
    fn location(&self) -> Point<i32, Physical> {
        self.location
    }
    fn size(&self) -> Size<i32, Physical> {
        self.surface.size
    }
    fn scale_factor(&self) -> f32 {
        self.scale_factor
    }
    fn set_location(&mut self, p: Point<i32, Physical>) {
        if self.location != p {
            self.location = p;
            // Mark changed so the move is picked up (damage), not rendered stale.
            self.commit.increment();
        }
    }
    fn queue_input(&mut self, input: Input) {
        self.runtime.queue_input(input);
    }
    fn has_queued(&self) -> bool {
        self.runtime.has_queued()
    }
    fn has_input(&self) -> bool {
        self.runtime.has_input()
    }
    fn tick_offscreen(&mut self) {
        self.runtime.tick_offscreen();
    }
    fn tick(&mut self) -> bool {
        let changed = self.runtime.tick();
        if changed {
            trace!("tick changed handle={:?}", self.id);
        }
        changed
    }
    fn acknowledge_frame(&mut self) {
        self.runtime.acknowledge_frame();
    }
    fn set_suspended(&mut self, suspended: bool) {
        self.runtime.set_suspended(suspended);
    }
    fn wants_frame(&self) -> bool {
        // `has_pending` is what keeps a pipelined publish reachable: the runtime
        // goes clean the moment it rasterizes, so without this the frame that
        // would retire the buffer is never scheduled.
        self.runtime.is_dirty() || self.surface.has_pending()
    }
    fn deadline(&self) -> Option<Instant> {
        self.runtime.deadline()
    }
    fn render(&mut self) {
        // No-op while the backing is released; the registry re-`ensure`s an
        // on-screen surface before calling this, so the view is present then.
        self.poll_publish();
        if let Some(view) = self.surface.begin_render_view() {
            self.runtime.render_into(&view);
            crate::frame_trace::rendered(self.id.0);
            self.surface
                .submitted(model::environment::interface::base::get().pipeline);
        }
        // `submitted` publishes inline when the ring cannot defer, so this is
        // what carries the commit bump on the non-pipelined path.
        self.poll_publish();
    }
    fn poll_publish(&mut self) {
        self.surface.poll();
        if self.generation != self.surface.generation() {
            self.generation = self.surface.generation();
            self.commit.increment();
        }
    }
    fn is_resident(&self) -> bool {
        self.surface.is_resident()
    }
    fn release_backing(&mut self) {
        self.surface.release();
    }
    fn ensure_backing(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> Result<(), SurfaceError> {
        self.surface.ensure(render_node, wgpu_ctx, gles)
    }
    fn sync_depth(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
        want: usize,
    ) -> Result<(), SurfaceError> {
        self.surface.sync_depth(render_node, wgpu_ctx, gles, want)
    }
    fn texture_handle(&self) -> Option<&smithay::backend::renderer::gles::GlesTexture> {
        self.surface.gles_texture()
    }
    fn apply_pending_resize(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> Result<bool, SurfaceError> {
        let Some((new_size, scale_factor)) = self.pending_resize.take() else {
            return Ok(false);
        };
        if new_size == self.surface.size && scale_factor == self.scale_factor {
            return Ok(false);
        }
        trace!(
            "resize handle={:?} old={:?} new={new_size:?}",
            self.id, self.surface.size
        );

        self.surface.resize(render_node, wgpu_ctx, gles, new_size)?;

        // println!("Scale factor just updated.");
        self.scale_factor = scale_factor;

        self.runtime
            .resize((new_size.w as u32, new_size.h as u32), self.scale_factor);
        self.commit.increment();
        Ok(true)
    }
    fn request_resize(&mut self, new_size: Size<i32, Physical>, scale_factor: f32) {
        // runtime doesnt keep track of instance scale. its safe to add
        self.pending_resize = Some((new_size, scale_factor));
    }
    fn pending_resize(&self) -> Option<(Size<i32, Physical>, f32)> {
        self.pending_resize
    }
    fn pointer_leave(&mut self) {
        self.runtime.pointer_leave();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ── UiItem (the registry's stored type) ─────────────────────────────

pub struct UiItem {
    pub(crate) inner: Box<dyn UiInstanceAny>,
    type_id: TypeId,
    space: UiSpace,
    pub layer: u64,
    /// When false the item is skipped by `elements`/`element_of` and never
    /// hit-tested. Lets a surface (e.g. a tooltip) be toggled on/off without
    /// reallocating its texture.
    visible: bool,
    /// When true the item is excluded from `hit_test`, so it floats above
    /// without intercepting pointer input or stealing events from what's
    /// behind it. Tooltips set this.
    passthrough: bool,
    /// When true a pointer press that lands on this surface does NOT move keyboard
    /// focus (neither clears the client's focus nor takes UI focus). The surface
    /// still receives the pointer button — it just isn't a keyboard-focus target.
    /// The on-screen keyboard sets this so tapping a key keeps the text field focused.
    keyboard_transparent: bool,
    /// Optional owning physical output (an opaque caller-supplied tag, e.g. an
    /// `output_key`). `None` = the surface is not bound to a specific monitor
    /// and follows the compositor's default single-output placement (launcher,
    /// dialogs, cursor). `Some(tag)` = the surface belongs to exactly that
    /// output; the scene builder draws it only on that monitor and the pointer
    /// path hit-tests it only when the cursor is on that monitor. Lets a screen
    /// overlay (e.g. a per-monitor capture stop button) be replicated once per
    /// output without duplicating/mispositioning on the others.
    output: Option<String>,
    /// True when the surface's UI state has advanced (via `tick`) since it was
    /// last rasterized, because it was off-screen at the time and `render` was
    /// skipped. The next time it is on-screen, `process_frame` renders it once
    /// to resync the texture, then clears this. See `process_frame`.
    render_stale: bool,
    /// Whether this surface paints its whole rect opaquely, so it may occlude
    /// surfaces beneath it. Opt-in (default false ⇒ never occludes anything);
    /// set for e.g. launcher placeholders whose background is fully opaque.
    /// Occlusion itself is evaluated per-pane in `element_of` (compositing) and
    /// by area subtraction in `manage_backings` (backing lifecycle).
    opaque_occluder: bool,
    /// Last time this item was visible on some output. Its GPU backing is
    /// released once it has been continuously hidden/off-screen/occluded for
    /// longer than the grace period (see `process_frame`), and re-allocated on
    /// reveal. The grace avoids alloc/free thrash when a surface skims the edge.
    last_on_screen: Instant,
    /// Marked when this surface has been hidden long enough to be a release
    /// candidate; the registry batches all pending candidates and flushes them
    /// together (debounced). Cleared when it becomes visible again or is released.
    release_pending: bool,
    /// The two GPU allocations re-attempted every frame until they succeed — the
    /// backing re-allocation on reveal and the ring depth sync. Neither input
    /// changes when the allocation fails, so without a gate a standing refusal
    /// (out of GPU memory, a size the driver will not take) is re-issued once per
    /// surface per composite with a log line each time. Keyed on the request, so
    /// the next attempt waits for a resize or a depth change rather than a clock:
    /// a driver's "no" to a specific allocation is not a transient condition.
    backing_failed: Attempt<Size<i32, Physical>>,
    depth_failed: Attempt<(usize, Size<i32, Physical>)>,
    /// World items only. `Some(ss)` pins the on-screen size to `buffer / ss`
    /// instead of letting the camera zoom scale it, so the surface stays the same
    /// number of screen pixels at any zoom.
    ///
    /// `ss` is a SUPERSAMPLE factor: at 1.0 the buffer is the on-screen size and
    /// is blitted 1:1; at 2.0 the caller allocates twice the pixels (with a
    /// matching UI scale factor, so the layout is unchanged) and the compositor
    /// downscales — which is how small glyphs stop looking chewed.
    ///
    /// The alternative — the counter-scale every chrome overlay used to do, an
    /// item sized `native/zoom` with an UI factor of `1/zoom` — also holds the
    /// on-screen size constant, but by SHRINKING the texture: at 4× zoom it
    /// rasterizes a quarter-resolution buffer and the compositor upscales it 4×,
    /// which is why the toolbars turned to mush when zoomed in. Pinning here
    /// keeps a fixed, known-good texture size and drops the per-zoom dmabuf
    /// realloc with it. Only the world ANCHOR still moves with the camera.
    ///
    /// `None` by default: content surfaces (placeholders, group surfaces) are
    /// supposed to grow with the canvas.
    zoom_lock: Option<f32>,
}

impl UiItem {
    pub(crate) fn new<U: UiProgram>(inst: UiInstance<U>, space: UiSpace, layer: u64) -> Self {
        Self {
            type_id: TypeId::of::<UiInstance<U>>(),
            inner: Box::new(inst),
            space,
            layer,
            visible: true,
            passthrough: false,
            keyboard_transparent: false,
            output: None,
            render_stale: false,
            opaque_occluder: false,
            last_on_screen: Instant::now(),
            release_pending: false,
            backing_failed: Attempt::new(),
            depth_failed: Attempt::new(),
            zoom_lock: None,
        }
    }

    // ── Occlusion ─────────────────────────────────────────────────

    pub fn is_opaque_occluder(&self) -> bool {
        self.opaque_occluder
    }
    /// Mark whether this surface fully covers its rect opaquely (so it can
    /// occlude surfaces beneath it). See the `opaque_occluder` field.
    pub fn set_opaque_occluder(&mut self, opaque: bool) {
        self.opaque_occluder = opaque;
    }

    // ── Visibility & passthrough ──────────────────────────────────

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Show/hide without destroying. Bumps the commit counter so smithay
    /// damages the item's rect on the transition.
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.inner.bump_commit();
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.passthrough
    }

    pub fn set_passthrough(&mut self, passthrough: bool) {
        self.passthrough = passthrough;
    }

    /// Whether a pointer press on this surface should leave keyboard focus alone.
    pub fn is_keyboard_transparent(&self) -> bool {
        self.keyboard_transparent
    }

    pub fn set_keyboard_transparent(&mut self, v: bool) {
        self.keyboard_transparent = v;
    }

    /// The output this surface is bound to, if any (see the `output` field).
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Bind (or unbind, with `None`) this surface to a specific physical output.
    pub fn set_output(&mut self, output: Option<String>) {
        self.output = output;
    }

    /// True if the runtime still wants to be rendered next frame (dirty or
    /// mid-animation).
    pub fn wants_frame(&self) -> bool {
        self.inner.wants_frame()
    }

    /// When a waiting animation next needs a frame (see
    /// [`UiInstanceAny::deadline`]).
    pub fn deadline(&self) -> Option<Instant> {
        self.inner.deadline()
    }

    // ── Identity ──────────────────────────────────────────────────

    pub fn handle_id(&self) -> HandleId {
        self.inner.handle_id()
    }
    pub fn space(&self) -> UiSpace {
        self.space
    }

    /// Change the item's space at runtime. Bumps the commit counter so
    /// the old screen rect is damaged.
    pub fn set_space(&mut self, space: UiSpace) {
        if self.space != space {
            self.space = space;
            self.inner.bump_commit();
        }
    }

    // ── Stored geometry (in the item's own space) ────────────────

    /// Position in the item's own space (World coords if `space() == World`;
    /// physical screen pixels if `space() == Screen`).
    pub fn location(&self) -> Point<i32, Physical> {
        self.inner.location()
    }

    /// Natural (unscaled) size in physical pixels.
    pub fn size(&self) -> Size<i32, Physical> {
        self.inner.size()
    }

    pub fn set_location(&mut self, p: Point<i32, Physical>) {
        self.inner.set_location(p);
    }

    // ── Camera-aware geometry (passed transform + output size) ─────

    /// Compute this item's on-screen position given the camera transform
    /// and output size. For `Screen` items, returns the stored location
    /// unchanged; for `World` items, applies the transform.
    /// The factor between this item's BUFFER size and its on-screen size. The
    /// single definition every size/hit-test path below shares — they disagreed
    /// once and clicks landed off the widget they appeared to be on.
    pub fn world_zoom(&self, transform: &Transform) -> f64 {
        match (self.space, self.zoom_lock) {
            (UiSpace::Screen, _) => 1.0,
            (UiSpace::World, Some(ss)) => 1.0 / (ss.max(0.01) as f64),
            (UiSpace::World, None) => transform.zoom,
        }
    }

    /// Pin this world item's on-screen size, optionally supersampled (see
    /// `zoom_lock`). The caller is responsible for allocating a buffer `ss` times
    /// the intended on-screen size with a matching UI scale factor.
    pub fn set_zoom_lock(&mut self, lock: Option<f32>) {
        self.zoom_lock = lock;
    }

    pub fn zoom_lock(&self) -> Option<f32> {
        self.zoom_lock
    }

    pub fn screen_location(
        &self,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> Point<i32, Physical> {
        match self.space {
            UiSpace::Screen => self.inner.location(),
            UiSpace::World => {
                let world = self.inner.location();
                let s = transform
                    .world_to_screen(output_size, Point::from((world.x as f64, world.y as f64)));
                Point::from((s.x as i32, s.y as i32))
            }
        }
    }

    /// On-screen size: the buffer scaled by `world_zoom` (1.0 for Screen items
    /// and for zoom-locked World ones, the camera zoom otherwise).
    pub fn screen_size(&self, transform: &Transform) -> Size<i32, Physical> {
        let size = self.inner.size();
        let zoom = self.world_zoom(transform);
        if (zoom - 1.0).abs() < f64::EPSILON {
            return size;
        }
        Size::from(((size.w as f64 * zoom) as i32, (size.h as f64 * zoom) as i32))
    }

    /// Full on-screen rectangle (location + size, both transformed).
    pub fn screen_rect(
        &self,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> Rectangle<i32, Physical> {
        Rectangle::new(
            self.screen_location(transform, output_size),
            self.screen_size(transform),
        )
    }

    /// True if the given screen point falls inside this item's current
    /// screen rect.
    pub fn contains_screen_point(
        &self,
        point: Point<f64, Physical>,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> bool {
        let r = self.screen_rect(transform, output_size);
        let r = Rectangle::<f64, Physical>::new(
            (r.loc.x as f64, r.loc.y as f64).into(),
            (r.size.w as f64, r.size.h as f64).into(),
        );
        r.contains(point)
    }

    /// Translate a screen point to surface-local logical coords (texture
    /// natural pixel grid). None if outside the screen rect.
    pub fn local_coords(
        &self,
        screen_point: Point<f64, Physical>,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> Option<(f32, f32)> {
        let r = self.screen_rect(transform, output_size);
        let r_f = Rectangle::<f64, Physical>::new(
            (r.loc.x as f64, r.loc.y as f64).into(),
            (r.size.w as f64, r.size.h as f64).into(),
        );
        if !r_f.contains(screen_point) {
            return None;
        }
        Some(self.local_coords_unclamped(screen_point, transform, output_size))
    }

    /// [`Self::local_coords`] for any screen point, inside the rect or not: a
    /// pointer grab follows the pointer off the surface.
    pub fn local_coords_unclamped(
        &self,
        screen_point: Point<f64, Physical>,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> (f32, f32) {
        let r = self.screen_rect(transform, output_size);
        let local = (
            screen_point.x - r.loc.x as f64,
            screen_point.y - r.loc.y as f64,
        );
        // Map the on-screen offset into the surface's UI LOGICAL space — the
        // space UI lays out and hit-tests in (see `UiRuntime::tick`, which
        // builds the UI at `viewport.logical_size()`). The on-screen rect spans
        // `size × world_zoom` physical px while the logical extent is
        // `size / scale_factor`, so the offset scales by
        // `logical / screen = 1 / (world_zoom × scale_factor)`.
        //
        // Both factors matter. A zoom-locked overlay has world_zoom 1 and factor
        // 1 (1:1). A plain World surface scales by the camera zoom alone.
        // Dividing by the camera zoom ALONE mis-mapped a zoomed-out counter-
        // scaled surface — the selection toolbar took clicks at the wrong spot.
        let divisor = self.world_zoom(transform) * self.inner.scale_factor() as f64;
        ((local.0 / divisor) as f32, (local.1 / divisor) as f32)
    }

    // ── Untyped passthrough ───────────────────────────────────────

    pub fn queue_input(&mut self, input: Input) {
        self.inner.queue_input(input);
    }
    pub fn has_queued(&self) -> bool {
        self.inner.has_queued()
    }
    pub fn has_input(&self) -> bool {
        self.inner.has_input()
    }
    pub(crate) fn tick_offscreen(&mut self) {
        self.inner.tick_offscreen();
    }
    pub fn pointer_leave(&mut self) {
        self.inner.pointer_leave();
    }
    pub fn request_resize(&mut self, new_size: Size<i32, Physical>, scale_factor: f32) {
        // inner is instance. instance holds runtime
        self.inner.request_resize(new_size, scale_factor);
    }
    pub fn pending_resize(&self) -> Option<(Size<i32, Physical>, f32)> {
        self.inner.pending_resize()
    }
    pub fn commit(&self) -> CommitCounter {
        self.inner.commit()
    }

    // ── Type queries ──────────────────────────────────────────────

    pub fn is<U: UiProgram>(&self) -> bool {
        self.type_id == TypeId::of::<UiInstance<U>>()
    }

    pub fn get<U: UiProgram>(&self) -> Option<&UiInstance<U>> {
        if self.is::<U>() {
            self.inner.as_any().downcast_ref::<UiInstance<U>>()
        } else {
            None
        }
    }

    pub fn get_mut<U: UiProgram>(&mut self) -> Option<&mut UiInstance<U>> {
        if self.is::<U>() {
            self.inner.as_any_mut().downcast_mut::<UiInstance<U>>()
        } else {
            None
        }
    }

    // ── Internal ──────────────────────────────────────────────────

    pub(crate) fn tick(&mut self) -> bool {
        let due = self.inner.tick();
        if !due && !self.render_stale {
            crate::frame_trace::unchanged(self.handle_id().0);
        }
        due
    }
    pub(crate) fn render(&mut self) {
        self.inner.set_suspended(false);
        self.inner.render();
        self.render_stale = false;
    }
    /// Retire a pipelined frame without rasterizing. Runs for every item every
    /// frame — see [`UiInstanceAny::poll_publish`].
    pub(crate) fn poll_publish(&mut self) {
        self.inner.poll_publish();
    }
    /// Advance frame bookkeeping without rasterizing (off-screen path) and mark
    /// the texture stale so the next on-screen frame re-renders it.
    pub(crate) fn skip_render(&mut self) {
        self.inner.acknowledge_frame();
        // Off screen: its animations stop waking the loop until it is shown.
        self.inner.set_suspended(true);
        self.render_stale = true;
    }
    pub(crate) fn is_stale(&self) -> bool {
        self.render_stale
    }
    pub(crate) fn bump_commit(&mut self) {
        self.inner.bump_commit();
    }

    // ── Backing (memory) lifecycle ────────────────────────────────

    pub fn is_resident(&self) -> bool {
        self.inner.is_resident()
    }
    /// Note that the item is visible right now, resetting its release grace.
    pub(crate) fn mark_on_screen(&mut self, now: Instant) {
        self.last_on_screen = now;
    }
    pub(crate) fn last_on_screen(&self) -> Instant {
        self.last_on_screen
    }
    pub(crate) fn release_pending(&self) -> bool {
        self.release_pending
    }
    pub(crate) fn set_release_pending(&mut self, pending: bool) {
        self.release_pending = pending;
    }
    /// Free the GPU backing while keeping the runtime alive. Marks the item
    /// stale so it re-renders once re-allocated (its texture is gone), and
    /// clears any pending-release marker.
    pub(crate) fn release_backing(&mut self) {
        self.inner.release_backing();
        self.render_stale = true;
        self.release_pending = false;
    }
    /// Re-allocate the backing at the current size if it was released.
    ///
    /// Returns whether the item is usable this frame. `false` means the
    /// allocation failed OR is being backed off after a failure, and the caller
    /// must skip the item — same handling either way, which is why this reports
    /// a bool rather than the error: the error is logged here, rate-limited, so
    /// a standing failure cannot flood the log at the composite rate.
    pub(crate) fn ensure_backing(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> bool {
        if self.inner.is_resident() {
            self.backing_failed.succeeded();
            return true;
        }
        let request = self.inner.size();
        if !self.backing_failed.worth_trying(&request) {
            return false;
        }
        match self.inner.ensure_backing(render_node, wgpu_ctx, gles) {
            Ok(()) => {
                self.backing_failed.succeeded();
                true
            }
            Err(e) => {
                if self.backing_failed.failed(request) {
                    warn!(
                        "UI backing alloc failed handle={:?} at {request:?}: {e:?}; \
                           not retried until the surface is resized",
                        self.inner.handle_id()
                    );
                }
                false
            }
        }
    }
    /// Match the ring to the live setting — see [`UiInstanceAny::sync_depth`].
    /// Gated on the request, for the same reason as `ensure_backing`.
    pub(crate) fn sync_depth(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
        want: usize,
    ) {
        let request = (want, self.inner.size());
        if !self.depth_failed.worth_trying(&request) {
            return;
        }
        match self.inner.sync_depth(render_node, wgpu_ctx, gles, want) {
            Ok(()) => self.depth_failed.succeeded(),
            Err(e) => {
                if self.depth_failed.failed(request) {
                    warn!(
                        "UI ring resize failed handle={:?} to {request:?}: {e:?}; \
                           not retried until the depth or size changes",
                        self.inner.handle_id()
                    );
                }
            }
        }
    }

    /// True if the item is visible AND its on-screen rect intersects the given
    /// output viewport `(0,0)..output_size`. Because `process_frame` runs once
    /// per output with that output's own camera, OR-ing this across passes means
    /// a surface is rasterized iff at least one monitor actually shows it.
    pub(crate) fn intersects_viewport(
        &self,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> bool {
        if !self.visible {
            return false;
        }
        let rect = self.screen_rect(transform, output_size);
        let viewport =
            Rectangle::from_size(Size::from((output_size.w as i32, output_size.h as i32)));
        rect.overlaps(viewport)
    }

    /// Build a render element for this item in screen coordinates.
    pub(crate) fn element_in(
        &self,
        transform: &Transform,
        output_size: Size<f64, Physical>,
    ) -> Option<UiRenderElement> {
        // `None` when there is nothing to show yet: the backing may be released.
        // Drawing an unwritten buffer would flash garbage.
        let texture = self.inner.texture_handle()?.clone();
        let location = self.screen_location(transform, output_size);
        let world_zoom = self.world_zoom(transform);
        Some(UiRenderElement {
            texture,
            space: self.space,
            location,
            size: self.inner.size(),
            world_zoom,
            id: self.inner.smithay_id().clone(),
            commit_counter: self.inner.commit(),
            output: self.output.clone(),
        })
    }

    /// Apply a pending resize, and MARK THE ITEM STALE when one landed.
    ///
    /// A resize replaces every slot in the ring, so the buffer the compositor
    /// samples is a fresh allocation whose contents are undefined. The surface
    /// must therefore re-render, and saying so here is the only way it is
    /// guaranteed: a World surface is rasterized by `manage_backings`, which
    /// renders only `is_stale()` items, and nothing else in that path knows the
    /// buffer was swapped.
    ///
    /// It happened to work by a chain of side effects — `UiRuntime::resize`
    /// calls `invalidate_layout`, which sets `dirty`, which `tick` reports as
    /// due, which the World branch of `process_frame` turns into `skip_render`,
    /// which sets stale. Four hops, any of which could stop being true, to
    /// express something this function already knows for certain.
    pub(crate) fn apply_pending_resize(
        &mut self,
        render_node: &str,
        wgpu_ctx: &WgpuGlContext,
        gles: &mut GlesRenderer,
    ) -> Result<bool, SurfaceError> {
        let resized = self
            .inner
            .apply_pending_resize(render_node, wgpu_ctx, gles)?;
        if resized {
            self.render_stale = true;
        }
        Ok(resized)
    }
}

impl std::fmt::Debug for UiItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiItem")
            .field("handle", &self.handle_id())
            .field("space", &self.space)
            .field("location", &self.location())
            .field("size", &self.size())
            .finish()
    }
}

pub(crate) fn build_instance<U: UiProgram>(
    id: HandleId,
    ui: U,
    surface: UiSurface,
    gpu: Arc<WgpuGlContext>,
    location: Point<i32, Physical>,
    scale_factor: f32,
) -> UiInstance<U> {
    let size_px = (surface.size.w as u32, surface.size.h as u32);
    let gpu = crate::engine::Gpu {
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
        format: graphics::surface::FORMAT,
    };
    let runtime = UiRuntime::new(ui, size_px, scale_factor, gpu);
    UiInstance {
        scale_factor,
        id,
        smithay_id: Id::new(),
        commit: CommitCounter::default(),
        location,
        surface,
        runtime,
        pending_resize: None,
        generation: 0,
    }
}
