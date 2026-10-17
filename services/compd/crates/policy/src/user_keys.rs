//! The user-key layer: compd's half of the §6.0 rule that the user's keys
//! belong to the user.
//!
//! The user keys are bare F1–F12 and Right Ctrl, Right Alt or Right Shift
//! with an arrow ([`inputd_schema::is_user_key`]). On a MixOS VT compd reads
//! the keyboard itself, so it enforces the rule itself, ahead of its own
//! binding table ([`crate::bindings`]):
//!
//! - a user key the user bound fires the user's verb;
//! - a user key the user bound `passthrough` goes on to the next layer;
//! - every other user key is swallowed;
//! - no user key ever reaches a client, and no shipped default binds one
//!   (the default keymap is empty).
//!
//! The decision is inputd's own: the same [`Resolver`] and press-latched
//! [`Interceptor`], fed the side-exact evdev code of every key edge. A key
//! is decided once, at its press; its repeats and its release follow that
//! decision, whatever the modifiers or the keymap are by then.
//!
//! Pure: no Wayland, no clock, no file system. The host feeds key edges as
//! xkb keycodes (evdev + 8, what smithay's backends report), runs the
//! [`UserFire`]s it is handed, arms a repeat timer when told to, and reads
//! the keymap file through [`parse_keymap`].

use comp_model::snapshot::UserKeyRowSnapshot;
use inputd_core::inputd_schema::{
    self, InputKeymap, KEYMAP_SCHEMA_VERSION, PhysicalBinding, PhysicalStroke, RepeatPolicy,
    SideModifiers, codes,
};
use inputd_core::{BindError, Edge, Fire, Interceptor, Resolver};
use serde_json::Value;

/// The service a fired verb runs on inside compd, through the same code a
/// Bus `comp.*` request takes, rather than over the Bus.
pub const COMP_SERVICE: &str = "comp";

/// xkb keycodes are evdev codes plus this offset.
pub const XKB_OFFSET: u32 = 8;

/// A verb to run: the resolved target service, the verb and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserFire {
    /// The explicit `service`, else the verb's first dot-segment (inputd's
    /// `fire_target` rule).
    pub service: String,
    pub verb: &'static str,
    /// The row's args; always a JSON object (empty when absent).
    pub body: Value,
}

impl UserFire {
    fn from_fire(fire: Fire) -> Option<Self> {
        let verb = fire.verb.as_str();
        let service = match fire.service {
            Some(explicit) => explicit,
            None => verb.split('.').next().unwrap_or("").to_owned(),
        };
        if service.is_empty() {
            return None;
        }
        let body = match fire.args {
            Some(args) if args.is_object() => args,
            _ => Value::Object(serde_json::Map::new()),
        };
        Some(Self {
            service,
            verb,
            body,
        })
    }

    /// True when the verb runs inside compd ([`COMP_SERVICE`]).
    pub fn is_comp(&self) -> bool {
        self.service == COMP_SERVICE
    }
}

/// What the layer decided for one key edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserKeyEdge {
    /// The layer took the edge: no later layer and no client sees it.
    pub took: bool,
    /// Run this verb (a press of a bound user key, never under a lock).
    pub fire: Option<UserFire>,
    /// Arm the repeat timer with this token: the press latched a row whose
    /// repeat policy is `allow`. [`UserKeys::repeat`] answers each tick.
    pub repeat: Option<u64>,
}

/// The layer's state: the user's keymap, the locked-session keymap (empty:
/// every user key swallowed, nothing fires), the latches and the side-exact
/// modifiers, and the key that is repeating.
#[derive(Clone, Debug)]
pub struct UserKeys {
    resolver: Resolver,
    locked: Resolver,
    interceptor: Interceptor,
    /// `(evdev code, token)` of the held key whose row repeats.
    repeating: Option<(u16, u64)>,
    next_token: u64,
}

impl Default for UserKeys {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl UserKeys {
    /// The layer over `rows` (already admitted by [`parse_keymap`]).
    pub fn new(rows: Vec<PhysicalBinding>) -> Self {
        let mut resolver = Resolver::new(InputKeymap::default());
        resolver.replace_physical(rows);
        Self {
            resolver,
            locked: Resolver::new(InputKeymap::default()),
            interceptor: Interceptor::new(),
            repeating: None,
            next_token: 0,
        }
    }

    /// The user's rows in force.
    pub fn rows(&self) -> &[PhysicalBinding] {
        self.resolver.physical_rows()
    }

    /// Replace the rows (a keymap reload). Held keys keep their latches:
    /// the new rows apply from each key's next press. Returns whether the
    /// rows changed.
    pub fn replace(&mut self, rows: Vec<PhysicalBinding>) -> bool {
        if rows.as_slice() == self.rows() {
            return false;
        }
        self.resolver.replace_physical(rows);
        true
    }

    /// The side-exact modifiers held, as the layer has seen them.
    pub fn modifiers(&self) -> SideModifiers {
        self.interceptor.mods()
    }

    /// True when `xkb_keycode` is held and the layer swallowed its press.
    pub fn is_swallowed(&self, xkb_keycode: u32) -> bool {
        evdev(xkb_keycode).is_some_and(|code| self.interceptor.is_swallowed(code))
    }

    /// Forget every latch, the modifiers and the repeat: the seat lost the
    /// keyboard (a session pause), so no release for what is held will come.
    pub fn reset(&mut self) {
        self.interceptor.end();
        self.repeating = None;
    }

    /// Decide one key edge. `locked`: a session lock is in force, so every
    /// user key is swallowed and nothing fires.
    pub fn edge(&mut self, xkb_keycode: u32, pressed: bool, locked: bool) -> UserKeyEdge {
        let Some(code) = evdev(xkb_keycode) else {
            return UserKeyEdge::default();
        };
        // The last key pressed is the one that repeats, as on a keyboard.
        if pressed || self.repeating.is_some_and(|(held, _)| held == code) {
            self.repeating = None;
        }
        let resolver = if locked { &self.locked } else { &self.resolver };
        let fresh = !(self.interceptor.is_swallowed(code) || self.interceptor.is_emitted(code));
        let mods = self.interceptor.mods();
        let step = self.interceptor.key(resolver, code, i32::from(pressed));
        // An orphan (its press came before the layer saw the keyboard) goes
        // on unchanged, as every key did before this layer.
        if step.emit || step.orphan {
            return UserKeyEdge::default();
        }
        let mut out = UserKeyEdge {
            took: true,
            ..UserKeyEdge::default()
        };
        if locked || !pressed {
            return out;
        }
        out.fire = step.fire.and_then(UserFire::from_fire);
        if fresh
            && self.interceptor.is_swallowed(code)
            && resolver.resolve(code, mods, Edge::Repeat).verb.is_some()
        {
            self.next_token += 1;
            self.repeating = Some((code, self.next_token));
            out.repeat = Some(self.next_token);
        }
        out
    }

    /// One repeat tick for `token`. `Some` fires the latched verb again and
    /// keeps the timer; `None` stops it (the key went up, another key was
    /// pressed, or the layer was reset).
    pub fn repeat(&mut self, token: u64) -> Option<UserFire> {
        let (code, current) = self.repeating?;
        if current != token || !self.interceptor.is_swallowed(code) {
            return None;
        }
        let step = self.interceptor.key(&self.resolver, code, 2);
        step.fire.and_then(UserFire::from_fire)
    }

    /// `bindings.user`: the rows in force, in keymap order.
    pub fn snapshot(&self) -> Vec<UserKeyRowSnapshot> {
        self.rows()
            .iter()
            .map(|row| UserKeyRowSnapshot {
                chord: chord(&row.stroke),
                action: row.action.as_str(),
                service: row.service.clone(),
                args: row.args.clone(),
                repeat: match row.repeat {
                    RepeatPolicy::Ignore => "ignore",
                    RepeatPolicy::Allow => "allow",
                },
                passthrough: row.passthrough,
            })
            .collect()
    }
}

fn evdev(xkb_keycode: u32) -> Option<u16> {
    xkb_keycode
        .checked_sub(XKB_OFFSET)
        .and_then(|code| u16::try_from(code).ok())
}

/// The side-exact spelling of a stroke: `RightCtrl+Right`, `F5`.
pub fn chord(stroke: &PhysicalStroke) -> String {
    let m = stroke.modifiers;
    let mut parts: Vec<String> = [
        (m.left_ctrl, "LeftCtrl"),
        (m.right_ctrl, "RightCtrl"),
        (m.left_alt, "LeftAlt"),
        (m.right_alt, "RightAlt"),
        (m.left_shift, "LeftShift"),
        (m.right_shift, "RightShift"),
        (m.left_super, "LeftSuper"),
        (m.right_super, "RightSuper"),
    ]
    .into_iter()
    .filter(|&(held, _)| held)
    .map(|(_, name)| name.to_owned())
    .collect();
    parts.push(key_name(stroke.code));
    parts.join("+")
}

fn key_name(code: u16) -> String {
    if let Some(index) = codes::F_KEYS.iter().position(|&f| f == code) {
        return format!("F{}", index + 1);
    }
    match code {
        codes::KEY_UP => "Up".into(),
        codes::KEY_LEFT => "Left".into(),
        codes::KEY_RIGHT => "Right".into(),
        codes::KEY_DOWN => "Down".into(),
        other => format!("code:{other}"),
    }
}

/// A keymap file that loaded: the admitted rows, and one line per row that
/// was dropped and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedKeymap {
    pub rows: Vec<PhysicalBinding>,
    pub dropped: Vec<String>,
}

/// Parse a keymap.v1 document (`{"version":1,"physical":[...]}`), the
/// format inputd reads and writes.
///
/// A document-level fault (not JSON, not an object, a version other than
/// [`KEYMAP_SCHEMA_VERSION`], no `physical` array) is an `Err`: the caller
/// keeps its last good keymap. A bad row costs only that row. Each row goes
/// through inputd's own admission ([`Resolver::bind_physical`]: action
/// grammar, service name, args shape and size, the row ceiling; a later row
/// on the same stroke replaces an earlier one). A row on a stroke that is
/// not a user key is dropped: this layer binds only the user keys, and
/// every other stroke belongs to compd's table and the clients.
pub fn parse_keymap(text: &str) -> Result<LoadedKeymap, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err("not a JSON object".into());
    };
    match object.get("version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(KEYMAP_SCHEMA_VERSION) => {}
        Some(version) => {
            return Err(format!(
                "keymap version {version}; this compd reads {KEYMAP_SCHEMA_VERSION}"
            ));
        }
        None => return Err("missing or non-integer version".into()),
    }
    let Some(physical) = object.get("physical").and_then(Value::as_array) else {
        return Err("missing or non-array physical".into());
    };
    let mut admission = Resolver::new(InputKeymap::default());
    let mut dropped = Vec::new();
    for (index, raw) in physical.iter().enumerate() {
        let row = match serde_json::from_value::<PhysicalBinding>(raw.clone()) {
            Ok(row) => row,
            Err(error) => {
                dropped.push(format!("row {index}: malformed: {error}"));
                continue;
            }
        };
        if !inputd_schema::is_user_key(&row.stroke) {
            dropped.push(format!(
                "row {index}: {} is not a user key",
                chord(&row.stroke)
            ));
            continue;
        }
        let stroke = chord(&row.stroke);
        if let Err(error) = admission.bind_physical(row) {
            let reason = match error {
                BindError::InvalidAction => "invalid action",
                BindError::InvalidArgs => "args is not an object",
                BindError::ArgsTooLarge => "args too large",
                BindError::AtCapacity => "too many rows",
                BindError::InvalidService => "invalid service name",
            };
            dropped.push(format!("row {index}: {stroke}: {reason}"));
        }
    }
    Ok(LoadedKeymap {
        rows: admission.physical_rows().to_vec(),
        dropped,
    })
}

#[cfg(test)]
#[path = "user_keys_tests.rs"]
mod tests;
