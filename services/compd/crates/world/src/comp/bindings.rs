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
//!
//! The user keys come first ([`policy::user_keys`], §6.0): bare F1–F12 and
//! Right Ctrl/Alt/Shift + arrows, matched side-exact on the evdev code. A
//! user key fires the user's binding, goes on when the user bound it
//! passthrough, and is otherwise swallowed; it never reaches the table
//! below or a client. A `comp` verb runs in the Bus pass through the comp
//! verbs' own parser and code; any other verb is sent over the comp port's
//! Bus client, fire-and-forget. The keymap is the session user's keymap.v1
//! file ([`user_keymap_path`]), followed live by an inotify watch.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use policy::bindings::{
    BindingAction, BindingProfile, BindingState, KeyDisposition, Modifiers, ScreenshotLaunch,
};
use policy::user_keys::{UserFire, UserKeys, parse_keymap};
use smithay::input::keyboard::ModifiersState;

use crate::state::Loop;

/// Overrides the user keymap's path (tests, a second session).
pub const USER_KEYMAP_ENV: &str = "MIXOS_USER_KEYMAP";

/// The user keys' auto-repeat, which compd runs itself for a row with
/// `repeat: allow` (the client never sees the key to repeat it). The same
/// delay and rate the seat advertises (`dispatcher` seat factory: 600 ms,
/// 25 per second).
const REPEAT_DELAY: Duration = Duration::from_millis(600);
const REPEAT_INTERVAL: Duration = Duration::from_millis(40);

/// Bus fires waiting for the comp port. With no port nothing drains them,
/// so the queue is bounded and the oldest goes first.
const MAX_BUS_FIRES: usize = 32;
/// A Bus fire older than this when the port drains it is dropped.
const BUS_FIRE_TTL: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct Bindings {
    /// The user-key layer, ahead of the table.
    pub user: UserKeys,
    /// User-key verbs fired since start, and the last one.
    pub user_fired: u64,
    pub user_last: Option<&'static str>,
    user_keymap: Option<UserKeymap>,
    /// `comp` verbs waiting for the Bus pass.
    comp_fires: Vec<UserFire>,
    /// Other services' verbs waiting for the comp port.
    bus_fires: VecDeque<(Instant, UserFire)>,
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

    /// Queue a user key's verb: a `comp` verb for the Bus pass, any other
    /// for the comp port.
    pub fn queue_user_fire(&mut self, fire: UserFire) {
        self.user_fired = self.user_fired.saturating_add(1);
        self.user_last = Some(fire.verb);
        if fire.is_comp() {
            self.comp_fires.push(fire);
            return;
        }
        if self.bus_fires.len() >= MAX_BUS_FIRES {
            self.bus_fires.pop_front();
        }
        self.bus_fires.push_back((Instant::now(), fire));
    }

    /// The queued `comp` verbs, oldest first.
    pub fn take_comp_fires(&mut self) -> Vec<UserFire> {
        std::mem::take(&mut self.comp_fires)
    }

    /// The queued verbs for other services, oldest first; any queued
    /// longer than [`BUS_FIRE_TTL`] is dropped.
    pub fn take_bus_fires(&mut self) -> Vec<UserFire> {
        let now = Instant::now();
        self.bus_fires
            .drain(..)
            .filter(|(queued, _)| now.duration_since(*queued) <= BUS_FIRE_TTL)
            .map(|(_, fire)| fire)
            .collect()
    }

    /// Read the user keymap at `path` into the layer. A missing file means
    /// no bindings (every user key swallowed); an unreadable or invalid one
    /// keeps the keymap in force. Returns whether the rows changed.
    pub fn reload_user_keymap(&mut self, path: &Path) -> bool {
        let rows = match std::fs::read_to_string(path) {
            Ok(text) => match parse_keymap(&text) {
                Ok(loaded) => {
                    for dropped in &loaded.dropped {
                        warn!("user keymap {}: {dropped}", path.display());
                    }
                    loaded.rows
                }
                Err(reason) => {
                    warn!(
                        "user keymap {}: {reason}; keeping the keymap in force",
                        path.display()
                    );
                    return false;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                warn!(
                    "user keymap {}: {error}; keeping the keymap in force",
                    path.display()
                );
                return false;
            }
        };
        let count = rows.len();
        let changed = self.user.replace(rows);
        if changed {
            info!("user keymap {}: {count} user-key bindings", path.display());
        }
        changed
    }
}

/// The keymap file and the watch on its directory (kept alive here).
struct UserKeymap {
    path: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl std::fmt::Debug for UserKeymap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserKeymap")
            .field("path", &self.path)
            .field("watched", &self._watcher.is_some())
            .finish()
    }
}

/// The session user's keymap.v1 file: `$MIXOS_USER_KEYMAP`, else
/// `inputd/keymap.json` in the MixOS etc dir (for a user,
/// `$XDG_CONFIG_HOME/mixos/inputd/keymap.json`).
pub fn user_keymap_path() -> PathBuf {
    std::env::var_os(USER_KEYMAP_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            config::path(config::Dir::Etc)
                .join("inputd")
                .join("keymap.json")
        })
}

/// Load the user keymap and watch it. Called once, before the first key.
pub fn start_user_keys(lp: &mut Loop) {
    let path = user_keymap_path();
    lp.inner.comp.bindings.reload_user_keymap(&path);
    info!(
        "user keys: keymap {} ({} bindings)",
        path.display(),
        lp.inner.comp.bindings.user.rows().len()
    );
    let watcher = watch_user_keymap(lp, &path);
    lp.inner.comp.bindings.user_keymap = Some(UserKeymap {
        path,
        _watcher: watcher,
    });
}

/// An inotify watch on the keymap's directory (an editor's save replaces
/// the file), delivered to the loop through a channel: each create, write,
/// rename or removal of the file reloads it. Our own reads are access
/// events and are ignored, so a reload never triggers another.
fn watch_user_keymap(lp: &mut Loop, path: &Path) -> Option<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    use smithay::reexports::calloop::channel::{Event, channel};
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        warn!("user keymap {}: no directory to watch", path.display());
        return None;
    };
    // The directory may not exist yet on a fresh account: make it, so a
    // keymap written later is seen.
    if let Err(error) = std::fs::create_dir_all(dir) {
        warn!("user keymap {}: {error}; not followed live", dir.display());
        return None;
    }
    let (sender, source) = channel::<()>();
    let reload = path.to_path_buf();
    let inserted = lp
        .loop_handle
        .insert_source(source, move |event, _, lp: &mut Loop| {
            if let Event::Msg(()) = event
                && lp.inner.comp.bindings.reload_user_keymap(&reload)
            {
                lp.inner.comp.settings_changed("bindings", "bindings.user");
            }
        });
    if let Err(error) = inserted {
        warn!("user keymap watch source: {error}");
        return None;
    }
    let name = name.to_os_string();
    let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event
            .paths
            .iter()
            .any(|changed| changed.file_name() == Some(name.as_os_str()))
        {
            let _ = sender.send(());
        }
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!("user keymap watch: {error}; not followed live");
            return None;
        }
    };
    if let Err(error) = watcher.watch(dir, RecursiveMode::NonRecursive) {
        warn!("user keymap {}: {error}; not followed live", dir.display());
        return None;
    }
    Some(watcher)
}

/// Repeat a user key whose row allows it, while it stays held: one timer,
/// armed at the press, re-armed each tick, dropped when the layer says the
/// repeat is over (the key went up, another was pressed, a pause) or a
/// session lock began.
fn arm_repeat(lp: &mut Loop, token: u64) {
    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    let armed = lp.loop_handle.insert_source(
        Timer::from_duration(REPEAT_DELAY),
        move |_, _, lp: &mut Loop| {
            if super::session_lock::active(lp) {
                return TimeoutAction::Drop;
            }
            let bindings = &mut lp.inner.comp.bindings;
            match bindings.user.repeat(token) {
                Some(fire) => {
                    bindings.queue_user_fire(fire);
                    TimeoutAction::ToDuration(REPEAT_INTERVAL)
                }
                None => TimeoutAction::Drop,
            }
        },
    );
    if let Err(error) = armed {
        warn!("user-key repeat timer: {error}");
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
    // The user keys first: the user's binding, the user's passthrough, or
    // swallowed. Under a lock every user key is swallowed and nothing fires.
    let user = user_key(
        &mut lp.inner.comp.bindings,
        keycode,
        pressed,
        locked,
        modifier,
    );
    if user.took {
        if let Some(token) = user.repeat {
            arm_repeat(lp, token);
        }
        return true;
    }
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
        // carries it, and bare F9 is a user key the layer above takes first.
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

/// The user-key half of [`key`], on the comp state alone (the headless
/// harness drives it as the seat does): the layer's decision for one edge,
/// with its verb queued and the edge recorded for the injector. When it
/// took the edge, the caller arms `repeat` and stops: no later layer and no
/// client sees the key.
pub fn user_key(
    bindings: &mut Bindings,
    keycode: u32,
    pressed: bool,
    locked: bool,
    modifier: bool,
) -> policy::user_keys::UserKeyEdge {
    let mut edge = bindings.user.edge(keycode, pressed, locked);
    if edge.took {
        bindings.last_edge = Some(KeyEdge {
            took: true,
            modifier,
        });
        if let Some(fire) = edge.fire.take() {
            bindings.queue_user_fire(fire);
        }
    }
    edge
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
