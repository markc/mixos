// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Compositor key bindings on compd's human keyboard.
//!
//! The filter ([`policy::bindings`]) runs in seat's keyboard path,
//! after xkb has processed the key and before the engine's own shortcut routing and
//! the client: [`key`] decides, and a key it takes (the press and, by the
//! release rule, its release) never reaches a client. Physical and
//! Bus-injected human keys share that path; the agent seat is never
//! filtered (agent keys bypass the binding filter).
//!
//! The table exists once the Bus pass has named the profile ([`Bindings::
//! ensure`], `nested` or `kms-live`); before that every key is forwarded.
//! Actions that touch only this state or the session run here (the toggle,
//! the VT switch, the nested exit); window and workspace actions are queued
//! and run by the same Bus pass through the comp verbs' code
//! (`policy_host::control::apply_bindings`), so a chord and its verb share
//! one primitive.

use policy::bindings::{
    BindingAction, BindingProfile, BindingState, KeyDisposition, Modifiers, ScreenshotLaunch,
};
use smithay::input::keyboard::ModifiersState;

use crate::state::Loop;

#[derive(Debug, Default)]
pub struct Bindings {
    state: Option<BindingState>,
    /// Window/workspace actions waiting for the Bus pass.
    pending: Vec<BindingAction>,
    /// Print Screen presses waiting for the launcher (compd's executor).
    launches: Vec<ScreenshotLaunch>,
    /// Actions fired since start, and the last one (`compd.truth`).
    pub fired: u64,
    pub last: Option<&'static str>,
    /// The last human key edge [`key`] saw (the `key_handled` inputs for
    /// an injected key). Cleared by the injector before each edge.
    pub last_edge: Option<KeyEdge>,
}

/// One human key edge as the filter saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyEdge {
    /// A binding took it (its press acted, or its release was swallowed).
    pub took: bool,
    /// Its symbol is a modifier: a prefix for a later chord, handled even
    /// with no focused client.
    pub modifier: bool,
}

impl Bindings {
    /// Build the table for `profile` once (`bindings.profile`).
    pub fn ensure(&mut self, profile: &str) {
        if self.state.is_none() {
            self.state = Some(BindingState::for_profile(
                BindingProfile::from_name(profile),
                true,
            ));
        }
    }

    pub fn state(&self) -> Option<&BindingState> {
        self.state.as_ref()
    }

    /// The queued window/workspace actions, oldest first.
    pub fn take_pending(&mut self) -> Vec<BindingAction> {
        std::mem::take(&mut self.pending)
    }

    /// Queue a Print Screen launch.
    pub fn launch(&mut self, launch: ScreenshotLaunch) {
        self.launches.push(launch);
    }

    /// The queued Print Screen launches, oldest first.
    pub fn take_launches(&mut self) -> Vec<ScreenshotLaunch> {
        std::mem::take(&mut self.launches)
    }
}

/// One human key edge, after xkb. `keysym` is the layout-agnostic symbol
/// (`raw_latin_sym_or_raw_current_sym`); `modifiers` the shortcut view
/// (nested: Right Ctrl stands in for Super). Returns whether compd took the
/// key (the client must not see it).
pub fn key(
    lp: &mut Loop,
    keycode: u32,
    pressed: bool,
    keysym: Option<u32>,
    modifiers: &ModifiersState,
) -> bool {
    let modifiers = Modifiers {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        logo: modifiers.logo,
        iso_level3_shift: modifiers.iso_level3_shift,
        iso_level5_shift: modifiers.iso_level5_shift,
        caps_lock: modifiers.caps_lock,
        num_lock: modifiers.num_lock,
    };
    let modifier =
        keysym.is_some_and(|sym| smithay::input::keyboard::Keysym::new(sym).is_modifier_key());
    // Under a session lock only the VT switch stays a binding, and a key no
    // lock surface holds the
    // keyboard for goes nowhere: not to the engine's shortcuts, the compositor's UI
    // or a client.
    let locked = super::session_lock::active(lp);
    let unheld = locked
        && !lp
            .state
            .seat
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .is_some_and(|surface| super::session_lock::is_lock_surface(lp, &surface));
    let bindings = &mut lp.inner.comp.bindings;
    bindings.last_edge = Some(KeyEdge {
        took: false,
        modifier,
    });
    let Some(state) = bindings.state.as_mut() else {
        return unheld;
    };
    let disposition = if locked {
        state.dispatch_session_locked(keycode, pressed, keysym, &modifiers)
    } else {
        state.dispatch(keycode, pressed, keysym, &modifiers)
    };
    let action = match disposition {
        KeyDisposition::Forward => return unheld,
        KeyDisposition::SwallowRelease => {
            bindings.last_edge = Some(KeyEdge {
                took: true,
                modifier,
            });
            return true;
        }
        KeyDisposition::Act(action) => action,
    };
    bindings.last_edge = Some(KeyEdge {
        took: true,
        modifier,
    });
    bindings.fired = bindings.fired.saturating_add(1);
    bindings.last = Some(action.name());
    match action {
        BindingAction::ToggleInterception => {
            let enabled = state.toggle_interception();
            info!("compositor key interception toggled: enabled={enabled}");
            // `bindings.enabled` moved.
            lp.inner.comp.settings_changed("bindings", "binding.toggle");
        }
        BindingAction::SwitchVt(vt) => switch_vt(lp, vt),
        BindingAction::ExitNestedCompositor => {
            info!("exit-nested-compositor binding: stopping the loop");
            lp.inner.loader.loop_signal.stop();
        }
        // The F9 Bus key is opt-in by configuration; compd's table never
        // carries it.
        BindingAction::SendBusKey => {}
        // Print Screen: the window holding the keyboard is read here, at the
        // key press, before any later input or queued action can move focus.
        BindingAction::Screenshot(mode) => {
            let focused = focused_window(lp);
            lp.inner
                .comp
                .bindings
                .launch(ScreenshotLaunch::new(mode, focused));
        }
        action => bindings.pending.push(action),
    }
    true
}

/// The window holding the human keyboard (its root, for a popup), as
/// `(id, generation)`, if it is a managed toplevel.
pub fn focused_window(lp: &Loop) -> Option<(u64, u64)> {
    use dispatcher::wire::trait_::surface_event::SurfaceHandle;
    let surface = lp.state.seat.seat.get_keyboard()?.current_focus()?;
    let registry = &lp.inner.comp.registry;
    let mut id = registry.id_for_handle(&SurfaceHandle::resolve(&surface))?;
    for _ in 0..64 {
        match registry.get(id).and_then(|record| record.parent()) {
            Some(parent) => id = parent,
            None => break,
        }
    }
    let record = registry.get(id)?;
    record
        .role()
        .managed_toplevel()
        .then(|| (record.id().0, record.generation()))
}

/// Ctrl+Alt+Fn on kms-live: the libseat VT switch, deferred to idle as the
/// engine's own VT handler does, and skipped while the session is already paused.
fn switch_vt(lp: &mut Loop, vt: u8) {
    lp.loop_handle.insert_idle(move |lp| {
        if let crate::state::state::StatusSession::Paused = lp.inner.status_session {
            return;
        }
        if let Some(session) = &mut lp.state.seat.libseat {
            use smithay::backend::session::Session;
            if let Err(err) = session.change_vt(i32::from(vt)) {
                warn!("VT switch to {vt} failed: {err:?}");
            }
        }
    });
}
