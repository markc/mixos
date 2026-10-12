//! `comp.region.select`, the modal part.
//!
//! While a selection runs, the HUMAN seat belongs to it: every button and key
//! is consumed (Escape or the right button cancels; a left drag selects that
//! rectangle; a click, Enter or keypad Enter selects the whole output; a line
//! re-arms), and pointer motion moves the cursor with NO client focus, so no
//! client sees enter, motion or a button. Releases of what was pressed during
//! the run are swallowed after it ends. Device input reaches this through the
//! human-input hook; injected human input (policy-host `input`) through the
//! same calls. Coordinates are output-local logical (the cursor's physical
//! screen position over the output's scale).
//!
//! The verb, its timers and its reply are the host's (policy-host `region`,
//! compd). The overlay is [`overlay`]: plain solids (the dim and the
//! selection's outline) and one CPU-rasterised hint, drawn only while a
//! selection runs on the output being drawn, plus the clean-frame record the
//! selected reply waits for (a selection is never claimed without a frame
//! free of its overlay).

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Instant;

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{
    AbsolutePositionEvent, ButtonState, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::input::pointer::MotionEvent;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Size};

use dispatcher::state::state::RedrawReason;

use crate::camera::transform::translate::transform::Transform;
use crate::state::Loop;
use crate::state::state::CoordinateTrait;

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Output-local logical `[x, y, width, height]`.
    Selected([i32; 4]),
    Cancelled(&'static str),
    Timeout,
    Busy,
    /// Refused with this error (the output changed under the run).
    Refused(&'static str),
    /// A session lock began (the run ends `locked`).
    Locked,
}

#[derive(Debug)]
pub struct Run {
    pub id: u64,
    /// The output's generation when the run started.
    pub generation: u64,
    /// The output being selected on (smithay name).
    pub output: String,
    /// The output the caller named, if any (a re-armed run goes back to it).
    pub requested: Option<String>,
    /// The output's logical size.
    pub bounds: (f64, f64),
    pub scale: f64,
    /// The left button, while it is down.
    pub press: Option<Press>,
    pub pointer: (f64, f64),
    pub deadline: Instant,
    /// The latest a selected reply may wait for its clean frame.
    pub reply_deadline: Instant,
    pub result: Option<Outcome>,
    /// A frame was built without the overlay after the run finished.
    pub clean: bool,
    /// The keyboard focus the run displaced, restored when it finishes.
    prior_focus: Option<WlSurface>,
}

#[derive(Debug, Default)]
pub struct Region {
    pub run: Option<Run>,
    next_id: u64,
    /// Keys and buttons pressed while a run held the seat: their releases are
    /// swallowed, even after it ends.
    keys: HashSet<u32>,
    buttons: HashSet<u32>,
}

impl Region {
    /// A run holds the seat (not yet finished).
    pub fn suspended(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.result.is_none())
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Swallow the release of keys already down when the run started.
    pub fn hold_keys(&mut self, keys: impl IntoIterator<Item = u32>) {
        self.keys.extend(keys);
    }
}

/// Begin holding the seat for `run`: the keyboard leaves its client
/// (remembered for afterwards), the
/// pointer leaves every surface, and keys already down are swallowed when
/// they come up.
pub fn begin(lp: &mut Loop, mut run: Run) {
    let seat = lp.state.seat.seat.clone();
    let time = lp.inner.start_time.elapsed().as_millis() as u32;
    if let Some(keyboard) = seat.get_keyboard() {
        run.prior_focus = keyboard.current_focus();
        keyboard.set_focus(&mut lp.state, None, SERIAL_COUNTER.next_serial());
        let held: Vec<u32> = keyboard
            .pressed_keys()
            .into_iter()
            .map(|key| key.raw())
            .collect();
        lp.inner.comp.region.hold_keys(held);
    }
    if let Some(pointer) = seat.get_pointer() {
        let location = pointer.current_location();
        pointer.motion(
            &mut lp.state,
            None,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(&mut lp.state);
    }
    lp.inner.comp.corners.reset();
    lp.inner.comp.region.run = Some(run);
    lp.state.schedule_redraw(RedrawReason::Capture);
}

/// End the run with `outcome`: the seat goes
/// back (keyboard focus restored if its surface still lives). A selected run
/// still waits for a clean frame before the host replies.
pub fn finish(lp: &mut Loop, outcome: Outcome) {
    let Some(run) = lp.inner.comp.region.run.as_mut() else {
        return;
    };
    if run.result.is_some() {
        return;
    }
    run.result = Some(outcome);
    run.clean = false;
    let prior = run.prior_focus.take().filter(|surface| {
        use smithay::reexports::wayland_server::Resource;
        surface.is_alive()
    });
    if let Some(keyboard) = lp.state.seat.seat.get_keyboard() {
        keyboard.set_focus(&mut lp.state, prior, SERIAL_COUNTER.next_serial());
    }
    lp.state.schedule_redraw(RedrawReason::Capture);
}

/// The displayed output-local logical rectangle between two
/// points, rounded outwards and clipped to the output. A click or a line is
/// not a region.
pub fn normalise(bounds: (f64, f64), a: (f64, f64), b: (f64, f64)) -> Option<[i32; 4]> {
    if ![a.0, a.1, b.0, b.1].iter().all(|value| value.is_finite()) {
        return None;
    }
    let (ax, ay) = (a.0.clamp(0.0, bounds.0), a.1.clamp(0.0, bounds.1));
    let (bx, by) = (b.0.clamp(0.0, bounds.0), b.1.clamp(0.0, bounds.1));
    if ax == bx || ay == by {
        return None;
    }
    let (x, y) = (ax.min(bx).floor(), ay.min(by).floor());
    let (right, bottom) = (
        ax.max(bx).ceil().min(bounds.0),
        ay.max(by).ceil().min(bounds.1),
    );
    if right > f64::from(i32::MAX) || bottom > f64::from(i32::MAX) {
        return None;
    }
    let rect = [x as i32, y as i32, (right - x) as i32, (bottom - y) as i32];
    (rect[2] > 0 && rect[3] > 0).then_some(rect)
}

/// How far the pointer may travel between press and release (logical
/// pixels) and still count as a click, not a drag.
pub const CLICK_SLOP: f64 = 4.0;

/// The pointer has travelled far enough from `start` to be a drag. A
/// non-finite point counts as a drag, so [`normalise`] refuses it.
pub fn dragged(start: (f64, f64), pointer: (f64, f64)) -> bool {
    let distance = (pointer.0 - start.0).hypot(pointer.1 - start.1);
    distance.is_nan() || distance >= CLICK_SLOP
}

/// The whole output, exactly as a drag from corner to corner reports it.
pub fn whole(bounds: (f64, f64)) -> Option<[i32; 4]> {
    normalise(bounds, (0.0, 0.0), bounds)
}

/// The left button while it is down: where it went down (output-local
/// logical) and whether the pointer has ever left the click slop since. Once
/// a press is a drag it stays one, even back at its start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    pub start: (f64, f64),
    pub dragging: bool,
}

impl Press {
    pub fn new(start: (f64, f64)) -> Self {
        Self {
            start,
            dragging: false,
        }
    }

    /// The pointer moved to `to` with the button down.
    pub fn moved(&mut self, to: (f64, f64)) {
        self.dragging = self.dragging || dragged(self.start, to);
    }

    /// What releasing at `end` selects: the dragged rectangle, the whole
    /// output for a click, or nothing for a line (the selection stays armed).
    pub fn release(mut self, bounds: (f64, f64), end: (f64, f64)) -> Option<[i32; 4]> {
        self.moved(end);
        if self.dragging {
            normalise(bounds, self.start, end)
        } else {
            whole(bounds)
        }
    }
}

/// What a key press does during a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Cancel,
    /// Select the whole output.
    Whole,
}

/// xkb keycodes (evdev + 8): KEY_ESC, KEY_ENTER, KEY_KPENTER.
const KEY_ESCAPE: u32 = 1 + 8;
const KEY_ENTER: u32 = 28 + 8;
const KEY_KP_ENTER: u32 = 96 + 8;

/// The action of a pressed key (raw xkb keycode); other keys are swallowed
/// and do nothing.
pub fn key_action(keycode: u32) -> Option<KeyAction> {
    match keycode {
        KEY_ESCAPE => Some(KeyAction::Cancel),
        KEY_ENTER | KEY_KP_ENTER => Some(KeyAction::Whole),
        _ => None,
    }
}

/// The pointer moved to `position` (output-local logical) during a run: the
/// cursor follows with no client focus.
pub fn motion(lp: &mut Loop, position: (f64, f64)) {
    let Some(run) = lp.inner.comp.region.run.as_mut() else {
        return;
    };
    let position = (
        position.0.clamp(0.0, run.bounds.0),
        position.1.clamp(0.0, run.bounds.1),
    );
    run.pointer = position;
    if let Some(press) = run.press.as_mut() {
        press.moved(position);
    }
    let physical = Point::<f64, Physical>::from((position.0 * run.scale, position.1 * run.scale));
    lp.inner.pointer_mut().motion.x = physical.x;
    lp.inner.pointer_mut().motion.y = physical.y;
    let ctx = lp.size_ctx_all();
    let transform: Transform = (physical, ctx).into();
    let location = transform.into_storage_point_f64();
    let time = lp.inner.start_time.elapsed().as_millis() as u32;
    if let Some(pointer) = lp.state.seat.seat.get_pointer() {
        pointer.motion(
            &mut lp.state,
            None,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(&mut lp.state);
    }
    lp.state.schedule_redraw(RedrawReason::Cursor);
}

/// A button during a run, or the release of one pressed during it. Returns
/// whether the region consumed it.
pub fn button(lp: &mut Loop, button: u32, pressed: bool) -> bool {
    let region = &mut lp.inner.comp.region;
    if !region.suspended() {
        // Decided already (Escape or Enter mid-drag): the press is dropped and
        // its release, swallowed, selects nothing; nor does any release of a
        // button the run took after it ends.
        if let Some(run) = region.run.as_mut() {
            run.press = None;
        }
        return !pressed && region.buttons.remove(&button);
    }
    if pressed {
        region.buttons.insert(button);
    } else {
        region.buttons.remove(&button);
    }
    let Some(run) = region.run.as_mut() else {
        return true;
    };
    let decided = match (button, pressed) {
        (0x111, true) => Some(Outcome::Cancelled("right_button")),
        (0x110, true) => {
            // Clamped onto the output: a press always starts the selection,
            // even with the cursor pinned at the far edge.
            let p = run.pointer;
            if p.0.is_finite() && p.1.is_finite() {
                run.press = Some(Press::new((
                    p.0.clamp(0.0, run.bounds.0),
                    p.1.clamp(0.0, run.bounds.1),
                )));
            }
            None
        }
        (0x110, false) => match run
            .press
            .take()
            .map(|press| press.release(run.bounds, run.pointer))
        {
            // A drag selects its rectangle; a click the whole output.
            Some(Some(rect)) => Some(Outcome::Selected(rect)),
            Some(None) => {
                // A line keeps the selection armed for another drag.
                if let Some(requested) = run.requested.clone() {
                    run.output = requested;
                }
                None
            }
            None => None,
        },
        _ => None,
    };
    match decided {
        Some(outcome) => finish(lp, outcome),
        None => lp.state.schedule_redraw(RedrawReason::Capture),
    }
    true
}

/// A key during a run (raw xkb keycode), or the release of one pressed
/// during it. Escape cancels; Enter and keypad Enter select the whole output.
/// Returns whether the region consumed it.
pub fn key(lp: &mut Loop, keycode: u32, pressed: bool) -> bool {
    let region = &mut lp.inner.comp.region;
    if !region.suspended() {
        return !pressed && region.keys.remove(&keycode);
    }
    if pressed {
        region.keys.insert(keycode);
    } else {
        region.keys.remove(&keycode);
        return true;
    }
    match key_action(keycode) {
        Some(KeyAction::Cancel) => finish(lp, Outcome::Cancelled("escape")),
        Some(KeyAction::Whole) => {
            let rect = region.run.as_ref().and_then(|run| whole(run.bounds));
            if let Some(rect) = rect {
                finish(lp, Outcome::Selected(rect));
            }
        }
        None => {}
    }
    true
}

/// The human-input hook's call: a device event during (or just after) a run.
/// Returns whether the region consumed it.
pub fn device_input<I: InputBackend>(lp: &mut Loop, event: &InputEvent<I>) -> bool {
    match event {
        InputEvent::Keyboard { event } => key(
            lp,
            event.key_code().raw(),
            event.state() == KeyState::Pressed,
        ),
        InputEvent::PointerButton { event } => button(
            lp,
            event.button_code(),
            event.state() == ButtonState::Pressed,
        ),
        _ if !lp.inner.comp.region.suspended() => false,
        InputEvent::PointerMotionAbsolute { event } => {
            let (width, height) = lp.size_ctx_all().screen_size_physical;
            let size = smithay::utils::Size::<i32, smithay::utils::Logical>::from((
                width.round() as i32,
                height.round() as i32,
            ));
            let at = event.position_transformed(size);
            let scale = lp
                .inner
                .comp
                .region
                .run
                .as_ref()
                .map_or(1.0, |run| run.scale);
            motion(lp, (at.x / scale, at.y / scale));
            true
        }
        InputEvent::PointerMotion { event } => {
            let Some(run) = lp.inner.comp.region.run.as_ref() else {
                return true;
            };
            let delta = event.delta();
            let at = (run.pointer.0 + delta.x, run.pointer.1 + delta.y);
            motion(lp, at);
            true
        }
        // Scroll and touch reach nothing while a selection holds the seat.
        InputEvent::PointerAxis { .. }
        | InputEvent::TouchDown { .. }
        | InputEvent::TouchMotion { .. }
        | InputEvent::TouchUp { .. } => true,
        _ => false,
    }
}

/// What the hint was rasterised for: scale and output width (as bits) and
/// the look.
type HintKey = (u64, u64, Look);
/// The rasterised hint and its key (`None` when there was nothing to draw).
type HintCache = Option<(HintKey, Option<(MemoryRenderBuffer, Size<i32, Logical>)>)>;

thread_local! {
    /// The outline's four edges, then the dim's four pieces.
    static OVERLAY_IDS: [Id; 8] = std::array::from_fn(|_| Id::new());
    /// Rasterised once per scale, output width and look, then reused frame
    /// to frame.
    static HINT: RefCell<HintCache> = const { RefCell::new(None) };
    /// The look, resolved once from the theme.
    static LOOK: Look = Look::of(&toolkit::theme::Theme::embedded());
}

/// The hint's type, logical pixels and weight.
const HINT_SIZE: f32 = 14.0;
const HINT_WEIGHT: u16 = 500;
/// Padding round the text, its corner radius and its gap from the top of the
/// output, logical pixels.
const HINT_PAD: (f64, f64) = (16.0, 8.0);
const HINT_RADIUS: f64 = 10.0;
const HINT_TOP: f64 = 24.0;

/// The overlay's colours, straight-alpha sRGB, from the MixOS theme the
/// capture overlays are drawn in (`toolkit::style` over the embedded theme,
/// as `capture::style`): the outline is the theme's accent, the hint its
/// popover surface, window stroke and strong text. The dim is the capture
/// mask (`capture::style::DIM_MASK`): translucency, not a themed surface.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    accent: [f32; 4],
    back: [f32; 4],
    edge: [f32; 4],
    text: [f32; 4],
    dim: [f32; 4],
}

impl Look {
    fn of(theme: &toolkit::theme::Theme) -> Self {
        let visuals = toolkit::style::visuals(theme);
        let straight = |colour: egui::Color32| {
            colour
                .to_srgba_unmultiplied()
                .map(|channel| f32::from(channel) / 255.0)
        };
        Self {
            accent: straight(visuals.selection.bg_fill),
            back: straight(visuals.window_fill),
            edge: straight(visuals.window_stroke.color),
            text: straight(visuals.strong_text_color()),
            dim: straight(crate::surface::draw::capture::style::DIM_MASK),
        }
    }

    fn current() -> Self {
        LOOK.with(|look| *look)
    }
}

/// `colour` premultiplied, as a solid element takes it.
fn premultiplied([r, g, b, a]: [f32; 4]) -> [f32; 4] {
    [r * a, g * a, b * a, a]
}

/// One frame's region overlay, in physical screen pixels.
pub struct Overlay<R: Renderer> {
    /// The hint, centred near the top; drawn over the solids.
    pub hint: Option<MemoryRenderBufferRenderElement<R>>,
    /// The outline's edges, then the dim's pieces (front-most first).
    pub solids: Vec<SolidColorRenderElement>,
}

impl<R: Renderer> Overlay<R> {
    fn empty() -> Self {
        Self {
            hint: None,
            solids: Vec::new(),
        }
    }
}

/// The region overlay for one frame. While a selection is armed: the whole
/// output dimmed, a thin outline round it and the hint. While dragging: the
/// dim around the selection only, the selection's outline and the hint.
/// Nothing on another output, nothing otherwise. After a run finishes, the
/// first frame of the selected output built without it is recorded as the
/// clean frame its reply waits for: another monitor's frame proves nothing.
pub fn overlay<R>(lp: &mut Loop, renderer: &mut R, _size: Size<i32, Physical>) -> Overlay<R>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let Some((output, finished)) = lp
        .inner
        .comp
        .region
        .run
        .as_ref()
        .map(|run| (run.output.clone(), run.result.is_some()))
    else {
        return Overlay::empty();
    };
    if !drawn_on(lp, &output) {
        return Overlay::empty();
    }
    let Some(run) = lp.inner.comp.region.run.as_mut() else {
        return Overlay::empty();
    };
    if finished {
        run.clean = true;
        return Overlay::empty();
    }
    let run = &*run;
    let look = Look::current();
    let scale = run.scale;
    let px = |value: f64| (value * scale).round() as i32;
    let line = px(f64::from(crate::surface::draw::capture::style::BORDER_WIDTH)).max(1);
    let (width, height) = (px(run.bounds.0), px(run.bounds.1));
    // Until the press is a drag it still reads as "the whole output".
    let selection = run
        .press
        .filter(|press| press.dragging)
        .map(|press| press.start)
        .map(|start| {
            let (x, y) = (
                px(start.0.min(run.pointer.0)),
                px(start.1.min(run.pointer.1)),
            );
            let (right, bottom) = (
                px(start.0.max(run.pointer.0)),
                px(start.1.max(run.pointer.1)),
            );
            (x, y, (right - x).max(1), (bottom - y).max(1))
        });
    let (x, y, w, h) = selection.unwrap_or((0, 0, width, height));
    let rect =
        |x: i32, y: i32, w: i32, h: i32| Rectangle::new(Point::from((x, y)), Size::from((w, h)));
    let edges = [
        rect(x, y, w, line),
        rect(x, y + h - line, w, line),
        rect(x, y, line, h),
        rect(x + w - line, y, line, h),
    ];
    let dims = match selection {
        Some(_) => vec![
            rect(0, 0, width, y),
            rect(0, y + h, width, height - (y + h)),
            rect(0, y, x, h),
            rect(x + w, y, width - (x + w), h),
        ],
        None => vec![rect(0, 0, width, height)],
    };
    let solids = OVERLAY_IDS.with(|ids| {
        let solid = |id: &Id, area: Rectangle<i32, Physical>, colour: [f32; 4]| {
            SolidColorRenderElement::new(
                id.clone(),
                area,
                CommitCounter::default(),
                colour,
                Kind::Unspecified,
            )
        };
        let edges = edges.into_iter().zip(&ids[..4]).map(|(edge, id)| {
            let edge = Rectangle::new(
                edge.loc,
                Size::from((edge.size.w.max(1), edge.size.h.max(1))),
            );
            solid(id, edge, premultiplied(look.accent))
        });
        let dims = dims
            .into_iter()
            .zip(&ids[4..])
            .filter(|(piece, _)| piece.size.w > 0 && piece.size.h > 0)
            .map(|(piece, id)| solid(id, piece, premultiplied(look.dim)));
        edges.chain(dims).collect()
    });
    let room = run.bounds.0 - 2.0 * HINT_TOP;
    let hint = hint_buffer(scale, room, look).and_then(|(buffer, logical)| {
        let (buffer_w, buffer_h) = (px(f64::from(logical.w)), px(f64::from(logical.h)));
        let at = Point::<f64, Physical>::from((
            f64::from((width - buffer_w) / 2),
            f64::from(px(HINT_TOP)),
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at,
            &buffer,
            None,
            Some(Rectangle::from_size(
                (f64::from(buffer_w), f64::from(buffer_h)).into(),
            )),
            Some(logical),
            Kind::Unspecified,
        )
        .ok()
    });
    Overlay { hint, solids }
}

/// This scene pass draws the run's output (named `output`).
fn drawn_on(lp: &Loop, output: &str) -> bool {
    let selected = lp
        .inner
        .space_state()
        .state
        .outputs()
        .find(|candidate| candidate.name() == output)
        .map(crate::state::state::output_key);
    pass_draws(lp.inner.render_output.as_deref(), selected.as_deref())
}

/// A pass for `render_key` (`None`: a pass that draws every output) draws
/// the selected output, whose key is `selected` (`None`: it is gone). A
/// vanished output is drawn by no keyed pass, so nothing else stands in for
/// it, neither the overlay nor its clean frame.
fn pass_draws(render_key: Option<&str>, selected: Option<&str>) -> bool {
    match render_key {
        None => true,
        Some(key) => selected.is_some_and(|selected| selected == key),
    }
}

/// The hint as a texture `logical` pixels in size, rasterised at `scale` in
/// `look` for an output `max_width` logical pixels wide (elided to fit).
fn hint_buffer(
    scale: f64,
    max_width: f64,
    look: Look,
) -> Option<(MemoryRenderBuffer, Size<i32, Logical>)> {
    HINT.with_borrow_mut(|slot| {
        let key = (scale.to_bits(), max_width.to_bits(), look);
        if slot.as_ref().is_none_or(|(made, _)| *made != key) {
            let text = crate::strings::text("region-select-hint");
            let image = hint_image(&text, scale, max_width, &look).map(|image| {
                let buffer = MemoryRenderBuffer::from_slice(
                    &image.argb,
                    Fourcc::Argb8888,
                    image.size,
                    1,
                    smithay::utils::Transform::Normal,
                    None,
                );
                (buffer, image.logical)
            });
            *slot = Some((key, image));
        }
        slot.as_ref().and_then(|(_, image)| image.clone())
    })
}

/// A rasterised hint: premultiplied ARGB8888 (B, G, R, A in memory).
struct HintImage {
    argb: Vec<u8>,
    size: Size<i32, Buffer>,
    logical: Size<i32, Logical>,
}

/// `text` in a rounded pill, at `scale` physical pixels per logical pixel.
/// Its logical size is whole pixels and its pixel size that times `scale`,
/// so it lands on the screen unscaled.
fn hint_image(text: &str, scale: f64, max_width: f64, look: &Look) -> Option<HintImage> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let room = ((max_width - 2.0 * HINT_PAD.0) * scale) as f32;
    // A narrow output has no room for the hint (nor a non-finite width).
    if !(room.is_finite() && room >= 1.0) {
        return None;
    }
    let mask = decor::text::title_mask(
        text,
        &decor::layout::DecoFontFamily::SystemUi,
        HINT_SIZE,
        HINT_WEIGHT,
        room,
        scale as f32,
    )?;
    let logical = Size::<i32, Logical>::from((
        (f64::from(mask.width) / scale + 2.0 * HINT_PAD.0).ceil() as i32,
        (f64::from(mask.height) / scale + 2.0 * HINT_PAD.1).ceil() as i32,
    ));
    let width = (f64::from(logical.w) * scale).round() as u32;
    let height = (f64::from(logical.h) * scale).round() as u32;
    if width < mask.width || height < mask.height {
        return None;
    }
    let (left, top) = ((width - mask.width) / 2, (height - mask.height) / 2);
    let (half_w, half_h) = (width as f32 / 2.0, height as f32 / 2.0);
    let radius = ((HINT_RADIUS * scale) as f32).min(half_h).min(half_w);
    let edge = (scale as f32).round().max(1.0);
    let quantise = |value: f32| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut argb = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // The rounded box's coverage from its signed distance, and the
            // same box inset by the edge: the ring between them is the edge.
            let cover = rounded(x, y, half_w, half_h, radius, 0.0);
            let inner = rounded(x, y, half_w, half_h, radius, edge);
            let (fill, ring) = (look.back[3] * inner, look.edge[3] * (cover - inner));
            let back = fill + ring;
            let ink = if x >= left && y >= top && x - left < mask.width && y - top < mask.height {
                f32::from(mask.at(x - left, y - top)) / 255.0 * look.text[3]
            } else {
                0.0
            };
            // Text over the box, premultiplied.
            let channel = |c: usize| {
                look.text[c] * ink + (look.back[c] * fill + look.edge[c] * ring) * (1.0 - ink)
            };
            argb.extend_from_slice(&[
                quantise(channel(2)),
                quantise(channel(1)),
                quantise(channel(0)),
                quantise(ink + back * (1.0 - ink)),
            ]);
        }
    }
    Some(HintImage {
        argb,
        size: Size::from((width as i32, height as i32)),
        logical,
    })
}

/// Coverage of pixel (`x`, `y`) by a box centred at (`half_w`, `half_h`)
/// with corner `radius`, inset by `inset` pixels on every side.
fn rounded(x: u32, y: u32, half_w: f32, half_h: f32, radius: f32, inset: f32) -> f32 {
    let (half_w, half_h, radius) = (half_w - inset, half_h - inset, (radius - inset).max(0.0));
    if half_w <= 0.0 || half_h <= 0.0 {
        return 0.0;
    }
    let (cx, cy) = (
        x as f32 + 0.5 - (half_w + inset),
        y as f32 + 0.5 - (half_h + inset),
    );
    let dx = (cx.abs() - (half_w - radius)).max(0.0);
    let dy = (cy.abs() - (half_h - radius)).max(0.0);
    let outside = (cx.abs() - half_w).max(cy.abs() - half_h);
    let distance = if dx > 0.0 && dy > 0.0 {
        dx.hypot(dy) - radius
    } else {
        outside
    };
    (0.5 - distance).clamp(0.0, 1.0)
}

impl Region {
    /// No key or button the region took is still down (a new run may start).
    pub fn idle(&self) -> bool {
        self.run.is_none() && self.keys.is_empty() && self.buttons.is_empty()
    }
}

impl Run {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u64,
        generation: u64,
        output: String,
        requested: Option<String>,
        bounds: (f64, f64),
        scale: f64,
        pointer: (f64, f64),
        deadline: Instant,
        reply_deadline: Instant,
    ) -> Self {
        Self {
            id,
            generation,
            output,
            requested,
            bounds,
            scale,
            press: None,
            pointer,
            deadline,
            reply_deadline,
            result: None,
            clean: false,
            prior_focus: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: (f64, f64) = (1_280.0, 800.0);

    /// Press at `start`, move through `path`, release at its last point.
    fn gesture(bounds: (f64, f64), start: (f64, f64), path: &[(f64, f64)]) -> Option<[i32; 4]> {
        let mut press = Press::new(start);
        for at in path {
            press.moved(*at);
        }
        press.release(bounds, *path.last().unwrap_or(&start))
    }

    #[test]
    fn a_click_selects_the_whole_output_as_a_corner_to_corner_drag_does() {
        let full = normalise(BOUNDS, (0.0, 0.0), BOUNDS);
        assert_eq!(full, Some([0, 0, 1_280, 800]));
        assert_eq!(whole(BOUNDS), full);
        assert_eq!(gesture(BOUNDS, (0.0, 0.0), &[(640.0, 400.0), BOUNDS]), full);
        // No movement, and movement just under the slop, either way.
        assert_eq!(gesture(BOUNDS, (500.0, 300.0), &[]), full);
        assert_eq!(gesture(BOUNDS, (500.0, 300.0), &[(502.5, 297.0)]), full);
        assert_eq!(
            gesture(BOUNDS, (500.0, 300.0), &[(498.0, 301.0), (496.1, 300.0)]),
            full
        );
        // At the far edge (the pinned cursor) too.
        assert_eq!(gesture(BOUNDS, BOUNDS, &[BOUNDS]), full);
    }

    #[test]
    fn a_fractional_output_is_whole_the_same_way() {
        // 1280 x 800 physical at 1.5: the drag and the click agree.
        let bounds = (1_280.0 / 1.5, 800.0 / 1.5);
        let full = normalise(bounds, (0.0, 0.0), bounds);
        assert_eq!(full, Some([0, 0, 853, 533]));
        assert_eq!(gesture(bounds, (10.0, 10.0), &[(11.0, 11.0)]), full);
    }

    #[test]
    fn movement_at_the_slop_is_a_drag() {
        assert!(!dragged((10.0, 10.0), (13.9, 10.0)));
        assert!(dragged((10.0, 10.0), (14.0, 10.0)));
        assert!(dragged((10.0, 10.0), (13.0, 13.0)));
        assert_eq!(
            gesture(BOUNDS, (10.0, 10.0), &[(14.0, 14.0)]),
            Some([10, 10, 4, 4])
        );
        assert_eq!(
            gesture(BOUNDS, (700.0, 500.0), &[(550.0, 400.0), (400.0, 300.0)]),
            Some([400, 300, 300, 200])
        );
    }

    #[test]
    fn a_drag_that_comes_back_to_its_start_stays_a_drag() {
        // Out to (400, 300) and back within the slop: the drag's own tiny
        // rectangle, never the whole output.
        let back = gesture(BOUNDS, (100.0, 100.0), &[(400.0, 300.0), (102.0, 102.0)]);
        assert_eq!(back, Some([100, 100, 2, 2]));
        assert_ne!(back, whole(BOUNDS));
        // Exactly back on the start is a point, not a region: it stays armed.
        assert_eq!(
            gesture(BOUNDS, (100.0, 100.0), &[(400.0, 300.0), (100.0, 100.0)]),
            None
        );
        let mut press = Press::new((100.0, 100.0));
        press.moved((101.0, 101.0));
        assert!(!press.dragging);
        press.moved((400.0, 300.0));
        press.moved((100.0, 100.0));
        assert!(press.dragging, "the drag latches");
    }

    #[test]
    fn a_dragged_line_selects_nothing_and_stays_armed() {
        assert_eq!(gesture(BOUNDS, (100.0, 100.0), &[(300.0, 100.0)]), None);
        assert_eq!(gesture(BOUNDS, (100.0, 100.0), &[(100.0, 300.0)]), None);
    }

    #[test]
    fn a_non_finite_point_is_never_a_selection() {
        assert!(dragged((10.0, 10.0), (f64::NAN, 10.0)));
        assert_eq!(gesture(BOUNDS, (10.0, 10.0), &[(f64::NAN, 10.0)]), None);
        assert_eq!(gesture(BOUNDS, (f64::INFINITY, 10.0), &[(10.0, 10.0)]), None);
    }

    #[test]
    fn enter_and_keypad_enter_select_the_whole_output_and_escape_cancels() {
        // xkb keycodes: evdev KEY_ENTER 28, KEY_KPENTER 96, KEY_ESC 1, plus 8.
        assert_eq!(key_action(36), Some(KeyAction::Whole));
        assert_eq!(key_action(104), Some(KeyAction::Whole));
        assert_eq!(key_action(9), Some(KeyAction::Cancel));
        // Space, a letter, F1 and Right Ctrl do nothing.
        for other in [65, 38, 67, 105] {
            assert_eq!(key_action(other), None, "keycode {other}");
        }
    }

    #[test]
    fn an_empty_output_has_no_whole() {
        assert_eq!(whole((0.0, 0.0)), None);
    }

    #[test]
    fn only_the_selected_outputs_pass_draws_the_overlay_or_proves_it_clean() {
        // A single pass draws every output.
        assert!(pass_draws(None, Some("A 1 x")));
        assert!(pass_draws(None, None));
        // The selected output's own pass.
        assert!(pass_draws(Some("A 1 x"), Some("A 1 x")));
        // Another monitor's pass: neither the overlay nor a clean frame.
        assert!(!pass_draws(Some("B 2 y"), Some("A 1 x")));
        // The selected output is gone (hotplug): no keyed pass stands in.
        assert!(!pass_draws(Some("B 2 y"), None));
    }

    #[test]
    fn the_look_comes_from_the_theme() {
        let look = Look::current();
        let visuals = toolkit::style::visuals(&toolkit::theme::Theme::embedded());
        let accent = visuals.selection.bg_fill.to_srgba_unmultiplied();
        assert_eq!(look.accent.map(|c| (c * 255.0).round() as u8), accent);
        assert!(look.back[3] > 0.0 && look.text[3] > 0.0);
        assert!(look.dim[3] > 0.0 && look.dim[3] < 1.0, "the dim is translucent");
    }

    #[test]
    fn the_hint_is_a_whole_logical_size_at_its_scale() {
        let text = crate::strings::text("region-select-hint");
        assert!(text.starts_with("Drag to select"), "{text}");
        let look = Look::current();
        let text_blue = (look.text[2] * 255.0) as i32;
        for scale in [1.0, 1.25, 2.0] {
            let image = hint_image(&text, scale, 1_280.0, &look).expect("a hint");
            let (w, h) = (image.size.w, image.size.h);
            assert_eq!(w, (f64::from(image.logical.w) * scale).round() as i32);
            assert_eq!(h, (f64::from(image.logical.h) * scale).round() as i32);
            assert_eq!(image.argb.len(), (w * h * 4) as usize);
            assert!(image.logical.w <= 1_280 - 2 * 24, "{:?}", image.logical);
            // The corner is clear, the middle row carries the box and ink.
            assert_eq!(image.argb[3], 0);
            let row = (h / 2) as usize * w as usize * 4;
            let alphas = (0..w as usize).map(|x| image.argb[row + x * 4 + 3]);
            assert!(alphas.clone().all(|a| a > 0));
            assert!(
                (0..w as usize).any(|x| {
                    let at = row + x * 4;
                    image.argb[at + 3] > 200 && (i32::from(image.argb[at]) - text_blue).abs() < 60
                }),
                "no text on the middle row"
            );
        }
        // A narrow output elides rather than overflows.
        let narrow = hint_image(&text, 1.0, 200.0, &look).expect("an elided hint");
        assert!(narrow.logical.w <= 200);
        // No room at all: no hint.
        assert!(hint_image(&text, 1.0, 2.0 * HINT_PAD.0, &look).is_none());
        assert!(hint_image(&text, 1.0, 10.0, &look).is_none());
    }
}
