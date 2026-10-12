//! Presentation-feedback collection, frame callbacks, and post-frame
//! housekeeping. Replaces the `refresh()` blocks duplicated in both backends.

use smithay::backend::renderer::element::RenderElementStates;
use smithay::desktop::utils::{
    OutputPresentationFeedback, surface_presentation_feedback_flags_from_states,
};
use smithay::desktop::{Window, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use std::time::Duration;
use world::state::Loop;

/// The presentation Kind flags this compositor reports for a hardware flip.
///
/// `Vsync` asserts synchronization to the vertical retrace, so it MUST be dropped
/// for an async-flipped frame — clients (Mesa's WSI, engine frame pacers) read it
/// to detect tearing and adapt their pacing, and reporting it on a torn frame
/// feeds them a false signal.
///
/// `HwClock` and `HwCompletion` stay set for BOTH, deliberately. Neither claims
/// anything about retrace: they say the timestamp came from the display hardware
/// and that the hardware signalled the presentation, and an async flip satisfies
/// both — the kernel's page-flip event IS the hardware saying the new buffer
/// began scanning out. Dropping them on a torn frame would replace one true
/// statement with a vaguer one; `Vsync` is the only flag that was ever wrong. The
/// other half of the same honesty is `Refresh::Unknown` at `wire.frame`: a torn
/// frame genuinely has no predictable next presentation.
pub fn hw_flip_kind(tearing: bool) -> wp_presentation_feedback::Kind {
    let hw = wp_presentation_feedback::Kind::HwClock | wp_presentation_feedback::Kind::HwCompletion;
    if tearing {
        hw
    } else {
        wp_presentation_feedback::Kind::Vsync | hw
    }
}

/// Collect presentation feedback for the windows visible in the frame about to be
/// queued. `states` are THIS frame's `RenderFrameResult::states`, threaded here
/// for one reason: `ZeroCopy` is per-SURFACE and unknowable at `presented()`
/// time, so it is the one flag that must be stored rather than passed.
pub fn collect_feedback(
    output: &Output,
    visible: &[Window],
    states: Option<&RenderElementStates>,
) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);
    for window in visible {
        window.take_presentation_feedback(
            &mut feedback,
            |_, _| Some(output.clone()),
            // `ZeroCopy` AND NOTHING ELSE: smithay ORs what is stored here into what
            // `presented()` passes (`utils.rs`: `callback.presented(..., flags |
            // self.flags)`) rather than replacing it, so a flag stored here can never
            // be cleared later. Storing `hw_flip_kind(false)` once pinned `Vsync` on
            // permanently — a torn frame passing `hw_flip_kind(true)` had it ORed
            // straight back in. `ZeroCopy` belongs here because it is a fact about
            // THIS surface in THIS frame that per-output `presented()` cannot know.
            |s, _| {
                states.map_or(wp_presentation_feedback::Kind::empty(), |st| {
                    surface_presentation_feedback_flags_from_states(s, None, st)
                })
            },
        );
    }
    feedback
}

/// Send frame callbacks to the visible windows. `throttle` follows the
/// caller's existing behavior (`Some(Duration::ZERO)` in both backends today).
///
/// An occluded window (on a pane, covered, not drawn: F6) is not in `visible`.
/// It is counted as a withheld opportunity here and gets the trickle
/// instead ([`trickle`]): one callback at most every [`OCCLUDED_TRICKLE`].
pub fn send_window_frames(state: &Loop, output: &Output, visible: &[Window]) {
    let frame_time = state.inner.start_time.elapsed();
    for window in visible {
        window.send_frame(output, frame_time, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
    let occluded = world::window::draw::occlude::record::occluded();
    world::window::draw::occlude::record::note_withheld(occluded.len());
    trickle(state);
    pace_thumbnails(state);
}

/// How often a window shown only as a hover thumbnail gets a frame callback.
/// A preview needs to move, not to run at the output's refresh rate; a window
/// also drawn in place gets its callbacks there at full rate.
pub const THUMBNAIL_FRAME: Duration = Duration::from_millis(33);

thread_local! {
    /// A thumbnail pacing timer is armed (one at a time).
    static THUMBNAIL_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The previewed windows waiting on a frame callback, each with the output
/// its tip is on. None under a session lock (only the lock surface gets
/// callbacks then).
fn thumbnail_targets(state: &Loop) -> Vec<(Window, Output)> {
    if world::comp::session_lock::active(state) {
        return Vec::new();
    }
    let space = &state.inner.space_state().state;
    world::window::draw::thumbnail::requests()
        .into_iter()
        .filter_map(|request| {
            let window = world::window::draw::frame::scene::capture_window(
                state,
                request.id,
                request.generation,
            )
            .ok()?;
            let output = space
                .outputs()
                .find(|o| world::state::state::output_key(o) == request.output)?
                .clone();
            awaits_frame(&window).then_some((window, output))
        })
        .collect()
}

/// Pace the windows a hover tip previews: a frame callback at most every
/// [`THUMBNAIL_FRAME`], from a timer, so the preview stays live without the
/// window joining the presented set (and driving frames at full rate).
/// Armed from each presented frame while a previewed window waits; re-arms
/// itself while one still does.
pub fn pace_thumbnails(state: &Loop) {
    if THUMBNAIL_ARMED.get() || thumbnail_targets(state).is_empty() {
        return;
    }
    let armed = state.loop_handle.insert_source(
        smithay::reexports::calloop::timer::Timer::from_duration(THUMBNAIL_FRAME),
        |_, _, state: &mut Loop| {
            THUMBNAIL_ARMED.set(false);
            let frame_time = state.inner.start_time.elapsed();
            for (window, output) in thumbnail_targets(state) {
                window.send_frame(&output, frame_time, Some(THUMBNAIL_FRAME), |_, _| {
                    Some(output.clone())
                });
            }
            let _ = state.inner.loader.display_handle.flush_clients();
            pace_thumbnails(state);
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        },
    );
    match armed {
        Ok(_) => THUMBNAIL_ARMED.set(true),
        Err(error) => warn!("thumbnail frame pacing not armed: {error}"),
    }
}

/// The occluded trickle: a covered window still gets a frame callback
/// this often, so a client that paces on callbacks (video, games, anything on
/// `wp_presentation`) keeps running instead of stalling until it is uncovered.
pub const OCCLUDED_TRICKLE: Duration = Duration::from_secs(1);

thread_local! {
    /// A trickle timer is armed (one at a time).
    static TRICKLE_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether any surface in `window`'s tree, or its popups' trees, holds a frame
/// callback the client is waiting on.
fn awaits_frame(window: &Window) -> bool {
    use smithay::wayland::seat::WaylandFocus;
    window
        .wl_surface()
        .is_some_and(|surface| tree_awaits_frame(&surface))
}

/// [`surface_awaits_frame`] for `surface` and each of its popups: the trees
/// `Window::send_frame` and `LayerSurface::send_frame` answer. A popup is its
/// own tree, so a popup-only wait is invisible to the root check alone.
pub fn tree_awaits_frame(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> bool {
    surface_awaits_frame(surface)
        || smithay::desktop::PopupManager::popups_for_surface(surface)
            .any(|(popup, _)| surface_awaits_frame(popup.wl_surface()))
}

/// Whether any surface in `surface`'s tree holds a frame callback the client is
/// waiting on.
pub fn surface_awaits_frame(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> bool {
    use smithay::wayland::compositor::{
        SurfaceAttributes, TraversalAction, with_surface_tree_downward,
    };
    // A Cell: the visit closure sets it and the continue predicate reads it.
    let pending = std::cell::Cell::new(false);
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            if !states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .is_empty()
            {
                pending.set(true);
            }
        },
        |_, _, _| !pending.get(),
    );
    pending.get()
}

/// The windows the trickle answers, with an output to attribute each callback
/// to: occluded windows (covered on a pane), and windows the comp policy hides
/// (minimised, or on a workspace that is not current). A hidden window is on no
/// pane, so the occlusion record calls it `unknown` and never lists it; without
/// this it got no callback at all, and a client that runs its UI loop on
/// callbacks (egui answering Bus verbs) stalled until it was shown again.
fn trickle_targets(state: &Loop) -> Vec<(Window, Output)> {
    use dispatcher::wire::trait_::surface_event::SurfaceHandle;
    use dispatcher::wire::trait_::wire_trait::WireTrait;
    use world::window::interface::record::window::LoopWindow;
    // Under a session lock only the lock surface gets callbacks (batch C), and
    // the cull is not run, so the record's decisions are stale: no trickle.
    if world::comp::session_lock::active(state) {
        return Vec::new();
    }
    let space = &state.inner.space_state().state;
    let mut targets: Vec<(Window, Output)> = world::window::draw::occlude::record::occluded()
        .into_iter()
        .filter_map(|(uuid, outputs)| {
            let window = space.elements().find(|w| w.uuid() == Some(uuid))?.clone();
            let output = space
                .outputs()
                .find(|o| {
                    outputs
                        .iter()
                        .any(|key| *key == world::state::state::output_key(o))
                })?
                .clone();
            awaits_frame(&window).then_some((window, output))
        })
        .collect();
    let comp = &state.inner.comp;
    let active = state.inner.active_output_key();
    let Some(output) = space
        .outputs()
        .find(|o| world::state::state::output_key(o) == active)
        .or_else(|| space.outputs().next())
        .cloned()
    else {
        return targets;
    };
    // Every world, not only the spawn target: a hidden client in a world that
    // is not hosted keeps its policy state and its pending callbacks.
    for world_space in state.inner.all_world_spaces() {
        for window in world_space.state.elements() {
            if SurfaceHandle::of_window(window).is_some_and(|handle| comp.hidden(&handle))
                && !targets.iter().any(|(w, _)| w == window)
                && awaits_frame(window)
            {
                targets.push((window.clone(), output.clone()));
            }
        }
    }
    targets
}

/// Arm the trickle when an occluded or hidden window waits on a callback: a
/// one-shot timer sends it (throttled by smithay to once per
/// [`OCCLUDED_TRICKLE`] per surface) and re-arms only while such a window
/// still waits. Never schedules a redraw: a window that stays idle costs no
/// frame and, once its callback is answered, no further wake. Called after
/// each presented frame and on every main-loop pass: a hidden window's commit
/// damages nothing, so no frame may follow it to arm the timer.
pub fn trickle(state: &Loop) {
    if TRICKLE_ARMED.get() || trickle_targets(state).is_empty() {
        return;
    }
    let armed = state.loop_handle.insert_source(
        smithay::reexports::calloop::timer::Timer::from_duration(OCCLUDED_TRICKLE),
        |_, _, state: &mut Loop| {
            TRICKLE_ARMED.set(false);
            let frame_time = state.inner.start_time.elapsed();
            for (window, output) in trickle_targets(state) {
                window.send_frame(&output, frame_time, Some(OCCLUDED_TRICKLE), |_, _| {
                    Some(output.clone())
                });
            }
            let _ = state.inner.loader.display_handle.flush_clients();
            trickle(state);
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        },
    );
    match armed {
        Ok(_) => TRICKLE_ARMED.set(true),
        Err(error) => warn!("occluded-window frame trickle not armed: {error}"),
    }
}

/// Send frame callbacks to the layers of ONE output — the one that just
/// presented. Multi-output: firing every output's layers on any output's flip
/// would pace a slow monitor's bar/panel at a fast neighbour's refresh; layer
/// surfaces are per-output (smithay keys layer maps by `Output`), so each
/// output drives only its own layers on its own vblank.
pub fn send_layer_frames(state: &Loop, output: &Output) {
    // Under a session lock only the lock surface
    // gets frame callbacks, and this presented lock frame counts toward `locked`.
    if world::comp::session_lock::presented(state, output) {
        return;
    }
    let frame_time = state.inner.start_time.elapsed();
    let layer_map = layer_map_for_output(output);
    for layer in layer_map.layers() {
        // A layer comp conceals gets no frame callbacks.
        if world::comp::panels::concealed(layer.wl_surface()) {
            continue;
        }
        layer.send_frame(output, frame_time, None, |_surface, _states| {
            Some(output.clone())
        });
    }
}

/// Post-frame housekeeping: space refresh, popup cleanup, client flush.
/// Runs every frame, damage or no damage.
///
/// `refresh_space` rather than smithay's `Space::refresh()`: the latter derives
/// `wl_output` enter/leave from the window's stored position, which here is a WORLD
/// coordinate and not a screen one — and no window belongs to one output anyway, since
/// every monitor renders the same world through its own camera.
pub fn housekeeping(state: &mut Loop) {
    state.inner.refresh_space();
    // After the frame, so it reads the presence stamps this frame just wrote.
    state.inner.refresh_suspended();
    state.state.popup.state.cleanup();
    let _ = state.inner.loader.display_handle.flush_clients();
}
