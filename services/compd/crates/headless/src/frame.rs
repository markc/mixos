// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! On-demand software presentation. No timer remains armed on an idle output.

use smithay::backend::renderer::{
    Bind, Offscreen, Texture,
    damage::OutputDamageTracker,
    gles::{GlesRenderer, GlesTexture},
};
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use world::state::Loop;

#[derive(Default)]
struct Retry {
    delay: Duration,
    warned: Option<Instant>,
}

impl Retry {
    /// Back off every failure, but emit at most one warning per five seconds.
    fn failed(&mut self, now: Instant, interval: Duration) -> bool {
        self.delay = (self.delay * 2).clamp(interval, Duration::from_secs(1));
        if self
            .warned
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
        {
            self.warned = Some(now);
            true
        } else {
            false
        }
    }
}

pub(crate) struct Context {
    output: Output,
    renderer: GlesRenderer,
    targets: [GlesTexture; 2],
    cursor: Option<GlesTexture>,
    next: usize,
    tracker: OutputDamageTracker,
    last: Option<Instant>,
    armed: bool,
    retry: Retry,
    interval: Duration,
}

impl Context {
    pub(crate) fn new(
        output: Output,
        renderer: GlesRenderer,
        targets: [GlesTexture; 2],
        tracker: OutputDamageTracker,
    ) -> Self {
        let refresh = output.current_mode().expect("headless mode").refresh;
        let interval = Duration::from_nanos(1_000_000_000_000 / refresh as u64);
        Self {
            output,
            renderer,
            targets,
            cursor: None,
            next: 0,
            tracker,
            last: None,
            armed: false,
            retry: Retry::default(),
            interval,
        }
    }
}

pub(crate) fn arm(state: &mut Loop, context: &Rc<RefCell<Context>>) {
    let mut ctx = context.borrow_mut();
    let key = world::state::state::output_key(&ctx.output);
    if ctx.armed || !state.state.redraw.needs(&key) {
        return;
    }
    let at = ctx.last.map_or_else(Instant::now, |last| {
        (last + ctx.interval.max(ctx.retry.delay)).max(Instant::now())
    });
    let context = Rc::clone(context);
    state
        .loop_handle
        .insert_source(Timer::from_deadline(at), move |_, _, state| {
            {
                let mut ctx = context.borrow_mut();
                ctx.armed = false;
                draw(state, &mut ctx);
            }
            arm(state, &context);
            TimeoutAction::Drop
        })
        .expect("headless: frame timer");
    ctx.armed = true;
}

fn draw(state: &mut Loop, ctx: &mut Context) {
    let key = world::state::state::output_key(&ctx.output);
    if !state.state.redraw.needs(&key) {
        return;
    }
    ctx.last = Some(Instant::now());
    state.state.redraw.rendering(&key);
    state.state.redraw.begin_render();
    state.inner.render_output = Some(key.clone());
    let mut visible = Vec::new();
    let result = compose(state, ctx, &mut visible);
    match result {
        Ok(true) => {
            ctx.retry.delay = Duration::ZERO;
            state.state.redraw.frame(&key, false);
            present(state, &ctx.output, &visible);
            model::stats::registry::base::present(&key);
            if slots::sdnotify::ready_once("first frame presented (headless)") {
                info!("headless: first offscreen frame completed");
            }
        }
        Ok(false) => {
            ctx.retry.delay = Duration::ZERO;
            state.state.redraw.frame(&key, true);
            // Empty frames owe callbacks too. This draw was already paced by
            // the one-shot timer, so no unbounded commit/callback loop arises.
            let mut feedback = frames::draw::present::callbacks::callbacks::collect_feedback(
                &ctx.output,
                &visible,
                None,
            );
            feedback.discarded();
            send_frames(state, &ctx.output, &visible);
        }
        Err(error) => {
            let now = Instant::now();
            ctx.last = Some(now);
            if ctx.retry.failed(now, ctx.interval) {
                warn!(
                    "headless: frame failed (retry in {:?}): {error}",
                    ctx.retry.delay
                );
            }
            ctx.tracker = OutputDamageTracker::from_output(&ctx.output);
            state
                .state
                .redraw
                .request_for(protocols::redraw::schedule::RedrawReason::Rescue);
        }
    }
    frames::draw::present::callbacks::callbacks::housekeeping(state);
    state.inner.render_output = None;
    state.state.redraw.end_render();
    if let Some(at) = graphics::bridge::publish::wake::take_deadline() {
        let handle = state.loop_handle.clone();
        state.state.redraw.request_at(
            &handle,
            at,
            protocols::redraw::schedule::RedrawReason::Ui,
            |s| &mut s.state.redraw,
        );
    }
}

fn compose(
    state: &mut Loop,
    ctx: &mut Context,
    visible: &mut Vec<Window>,
) -> Result<bool, Box<dyn std::error::Error>> {
    use frames::draw::plan::frame::frame::{FramePass, plan};
    let capture =
        screencopy::wants_full_frame(&ctx.output) || screencopy::file::pending(&ctx.output);
    if !plan(&state.inner.status).has_pass(FramePass::Scene) && !capture {
        return Ok(false);
    }
    let size = ctx.output.current_mode().expect("headless mode").size;
    let prepared = frames::scene::scene::prepare(state, &mut ctx.renderer, size);
    let scene = frames::scene::scene::scene(state, &mut ctx.renderer, size, prepared);
    *visible = scene.visible_window;
    // Track content changes independently of the alternating targets. This
    // age-1 query is only a scene diff; it never assumes preserved pixels in
    // either texture. An unchanged scene submits nothing.
    let damage = ctx
        .tracker
        .damage_output(1, &scene.Element)?
        .0
        .filter(|damage| !damage.is_empty())
        .cloned();
    let damaged = damage.is_some();
    let due = screencopy::sources_due(&ctx.output, damage.as_deref());
    if !damaged && !capture && !due.any() {
        return Ok(false);
    }

    let scale = ctx.output.current_scale().fractional_scale();
    // Window jobs use the owning window's scene (including parked worlds),
    // just as nested does. They never consume the output framebuffer.
    screencopy::offscreen::capture_windows(&mut ctx.renderer, &ctx.output, |renderer, target| {
        let window =
            world::window::draw::frame::scene::capture_window(state, target.id, target.generation)
                .map_err(|error| screencopy::file::ControlReply::WindowTarget {
                    id: target.id,
                    error,
                })?;
        world::window::draw::frame::scene::capture(renderer, &window, scale)
            .map_err(screencopy::file::capture_failed)
    });

    // The normal target is always cursorless. No second allocation or pass
    // occurs unless a protocol or Bus capture explicitly asks for the cursor.
    {
        let elements: Vec<_> = scene
            .Element
            .iter()
            .filter(|element| !element.is_cursor())
            .collect();
        let mut target = ctx.renderer.bind(&mut ctx.targets[ctx.next])?;
        // No buffer-age assumptions: each target is repainted completely.
        OutputDamageTracker::from_output(&ctx.output)
            .render_output(
                &mut ctx.renderer,
                &mut target,
                0,
                &elements,
                [0.1, 0.1, 0.1, 1.0],
            )?
            .sync
            .wait()?;
    }
    // Service may coalesce accumulated damage into a box that makes a waiting
    // copy due. Once drawing, offer the cursor for every pending cursor copy.
    let cursor_needed = screencopy::cursor_pending(&ctx.output)
        || screencopy::file::pending_picture(&ctx.output, true);
    if cursor_needed {
        let texture_size = (size.w, size.h).into();
        if ctx
            .cursor
            .as_ref()
            .is_none_or(|texture| texture.size() != texture_size)
        {
            ctx.cursor = Some(Offscreen::<GlesTexture>::create_buffer(
                &mut ctx.renderer,
                smithay::backend::allocator::Fourcc::Abgr8888,
                texture_size,
            )?);
        }
        {
            let mut target = ctx
                .renderer
                .bind(ctx.cursor.as_mut().expect("cursor texture"))?;
            OutputDamageTracker::from_output(&ctx.output)
                .render_output(
                    &mut ctx.renderer,
                    &mut target,
                    0,
                    &scene.Element,
                    [0.1, 0.1, 0.1, 1.0],
                )?
                .sync
                .wait()?;
        }
    }
    let cursor_target = ctx
        .cursor
        .as_mut()
        .filter(|_| cursor_needed)
        .map(|texture| ctx.renderer.bind(texture))
        .transpose()?;
    let target = ctx.renderer.bind(&mut ctx.targets[ctx.next])?;
    let readback = screencopy::Readback {
        size,
        origin_bottom_left: false,
    };
    // All copies start only after completion of both passes. Missing sources
    // fail explicitly rather than borrowing another capture's cursor policy.
    let captures = screencopy::service_with_sources(
        &mut ctx.renderer,
        screencopy::Sources {
            cursor: cursor_target
                .as_ref()
                .map(|framebuffer| screencopy::Source {
                    framebuffer,
                    readback,
                }),
            cursorless: Some(screencopy::Source {
                framebuffer: &target,
                readback,
            }),
        },
        &ctx.output,
        damage.as_deref(),
    );
    screencopy::file::service(
        &mut ctx.renderer,
        screencopy::Source {
            framebuffer: &target,
            readback,
        },
        &ctx.output,
        screencopy::file::CaptureSource::Offscreen,
        false,
    );
    if let Some(framebuffer) = cursor_target.as_ref() {
        screencopy::file::service(
            &mut ctx.renderer,
            screencopy::Source {
                framebuffer,
                readback,
            },
            &ctx.output,
            screencopy::file::CaptureSource::Offscreen,
            true,
        );
    }
    // finish() and file::service above synchronously map their readbacks;
    // GLES map_texture blocks on each copy (including file::read_pixels).
    // All reads complete before targets rotate or the cached cursor is reused.
    captures.finish(&mut ctx.renderer);
    ctx.next ^= 1;
    // A capture of unchanged content is not a new presentation.
    Ok(damaged)
}

fn present(state: &mut Loop, output: &Output, visible: &[Window]) {
    use frames::draw::present::{callbacks::callbacks, software::software};
    let mut feedback = callbacks::collect_feedback(output, visible, None);
    software::presented_now(&mut feedback, output);
    world::comp::presentation::frame_queued(state, output, visible);
    let now: Duration = smithay::utils::Clock::<smithay::utils::Monotonic>::new()
        .now()
        .into();
    world::comp::presentation::presented(
        state,
        output,
        now,
        None,
        software::software_present_kind().bits(),
    );
    send_frames(state, output, visible);
}

fn send_frames(state: &mut Loop, output: &Output, visible: &[Window]) {
    use frames::draw::present::callbacks::callbacks;
    callbacks::send_window_frames(state, output, visible);
    callbacks::send_layer_frames(state, output);
    frames::draw::present::cursor::cursor::send_frames(state, output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_errors_back_off_and_warnings_are_throttled() {
        let mut retry = Retry::default();
        let now = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 30);
        assert!(retry.failed(now, interval));
        assert_eq!(retry.delay, interval);
        assert!(!retry.failed(now + interval, interval));
        assert_eq!(retry.delay, interval * 2);
        for _ in 0..10 {
            assert!(!retry.failed(now + Duration::from_secs(1), interval));
        }
        assert_eq!(retry.delay, Duration::from_secs(1));
        assert!(retry.failed(now + Duration::from_secs(5), interval));
        // A good frame clears the delay; warning throttling survives recovery.
        retry.delay = Duration::ZERO;
        assert!(!retry.failed(now + Duration::from_secs(6), interval));
        assert_eq!(retry.delay, interval);
    }
}
