// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! inputd's pure core. No evdev, no Bus, no clock: the daemon feeds it the
//! facts and acts on its answers.
//!
//! - [`Resolver`] owns the keymap and the mode, and answers one question:
//!   for this key, with these side-specific modifiers, on this edge, does a
//!   command fire and is the stroke swallowed. A bound row swallows every edge
//!   of its stroke and fires on the press (and on repeats when its policy
//!   allows). A user key ([`inputd_schema::is_user_key`]) with no row is
//!   swallowed too, so no app ever contends for it.
//! - [`Interceptor`] applies those answers to a stream of key events under
//!   a grab, deciding each key **once, at its press** and holding that
//!   decision until its release ([`interceptor`]).

pub mod interceptor;

pub use inputd_schema;
pub use interceptor::{Fire, Interceptor, Step};

use inputd_schema::{
    ActionId, BindingScope, InputKeymap, InputMode, PhysicalBinding, PhysicalStroke, RepeatPolicy,
    SideModifiers, codes, is_user_key,
};

/// A key event's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Press,
    Repeat,
    Release,
}

/// The core's answer for one event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Resolution {
    /// The command to fire, on a firing edge of a bound row.
    pub command: Option<ActionId>,
    /// The row's args, present only with `command`.
    pub args: Option<serde_json::Value>,
    /// The row's explicit target, present only with `command`.
    pub service: Option<String>,
    /// Do not re-emit the original event.
    pub swallow: bool,
}

impl Resolution {
    /// Re-emit; fire nothing.
    pub const PASS: Self = Self {
        command: None,
        args: None,
        service: None,
        swallow: false,
    };
    /// Swallow; fire nothing.
    pub const SWALLOW: Self = Self {
        command: None,
        args: None,
        service: None,
        swallow: true,
    };
}

/// Why a rebind was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    /// The action id is outside the shared grammar.
    InvalidAction,
    /// `args` must be a JSON object, or absent.
    InvalidArgs,
    /// `args` serialised beyond [`MAX_ARGS_BYTES`].
    ArgsTooLarge,
    /// The keymap already holds [`MAX_PHYSICAL_ROWS`] rows.
    AtCapacity,
    /// `service` is not registered-name-shaped (`^[a-z][a-z0-9-]{1,30}$`).
    InvalidService,
}

/// True when `name` matches the broker's registered-service grammar,
/// `^[a-z][a-z0-9-]{1,30}$`. A row's `service` must pass it at bind and at
/// keymap load.
pub fn service_is_valid(name: &str) -> bool {
    (2..=31).contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The most physical rows a keymap holds.
pub const MAX_PHYSICAL_ROWS: usize = 512;

/// The largest serialised `args` one row may carry.
pub const MAX_ARGS_BYTES: usize = 4096;

/// The resolver: keymap, mode and rebind generation.
#[derive(Clone, Debug)]
pub struct Resolver {
    keymap: InputKeymap,
    mode: InputMode,
    generation: u64,
}

impl Resolver {
    /// A resolver over `keymap`, in [`InputMode::Normal`], at generation 0.
    pub fn new(keymap: InputKeymap) -> Self {
        Self {
            keymap,
            mode: InputMode::Normal,
            generation: 0,
        }
    }

    /// Bumped by every applied change, so a caller can see a map it raced.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn physical_rows(&self) -> &[PhysicalBinding] {
        &self.keymap.physical
    }

    /// Set the mode. Returns the generation, bumped when the mode changed.
    pub fn set_mode(&mut self, mode: InputMode) -> u64 {
        if self.mode != mode {
            self.mode = mode;
            self.generation += 1;
        }
        self.generation
    }

    /// Resolve one event. Deterministic in `(keymap, mode, code, mods,
    /// edge)`. The caller decides at the press and latches the answer
    /// ([`Interceptor`]); resolving a release on its own can give a
    /// different answer once the modifiers have changed.
    pub fn resolve(&self, code: u16, mods: SideModifiers, edge: Edge) -> Resolution {
        // The panic hatch passes everything, user keys included.
        if self.mode == InputMode::Transparent {
            return Resolution::PASS;
        }
        let stroke = PhysicalStroke {
            code,
            modifiers: mods,
        };
        let Some(binding) = self.match_physical(&stroke) else {
            // An unbound user key is the person's: swallowed, never an app's.
            return if is_user_key(&stroke) {
                Resolution::SWALLOW
            } else {
                Resolution::PASS
            };
        };
        if binding.passthrough {
            return Resolution::PASS;
        }
        let command = match edge {
            Edge::Press => Some(binding.action),
            Edge::Repeat if binding.repeat == RepeatPolicy::Allow => Some(binding.action),
            Edge::Repeat | Edge::Release => None,
        };
        Resolution {
            args: command.and(binding.args.clone()),
            service: command.and(binding.service.clone()),
            command,
            swallow: true,
        }
    }

    /// Exact match: the same key and the same side-specific modifiers.
    fn match_physical(&self, stroke: &PhysicalStroke) -> Option<&PhysicalBinding> {
        self.keymap
            .physical
            .iter()
            .find(|binding| binding.stroke == *stroke)
    }

    /// Admit a physical row: grammar, target, args and capacity checks,
    /// then replace any row on the same stroke. Returns the new generation.
    pub fn bind_physical(&mut self, binding: PhysicalBinding) -> Result<u64, BindError> {
        if ActionId::validate_str(binding.action.as_str()).is_err() {
            return Err(BindError::InvalidAction);
        }
        if binding
            .service
            .as_deref()
            .is_some_and(|name| !service_is_valid(name))
        {
            return Err(BindError::InvalidService);
        }
        if let Some(args) = &binding.args {
            if !args.is_object() {
                return Err(BindError::InvalidArgs);
            }
            let size = serde_json::to_string(args).map_or(usize::MAX, |s| s.len());
            if size > MAX_ARGS_BYTES {
                return Err(BindError::ArgsTooLarge);
            }
        }
        match self
            .keymap
            .physical
            .iter()
            .position(|row| row.stroke == binding.stroke)
        {
            Some(index) => self.keymap.physical[index] = binding,
            None => {
                if self.keymap.physical.len() >= MAX_PHYSICAL_ROWS {
                    return Err(BindError::AtCapacity);
                }
                self.keymap.physical.push(binding);
            }
        }
        self.generation += 1;
        Ok(self.generation)
    }

    /// Replace every row (a keymap file reload). Keeps the mode.
    pub fn replace_physical(&mut self, rows: Vec<PhysicalBinding>) -> u64 {
        self.keymap.physical = rows;
        self.generation += 1;
        self.generation
    }

    /// Remove the row on `stroke`. Returns the new generation if one went.
    pub fn unbind_physical(&mut self, stroke: &PhysicalStroke) -> Option<u64> {
        let before = self.keymap.physical.len();
        self.keymap.physical.retain(|row| row.stroke != *stroke);
        (self.keymap.physical.len() != before).then(|| {
            self.generation += 1;
            self.generation
        })
    }
}

/// The registered name of the scenes loader the scene-editor chord targets.
pub const SCENES_SERVICE: &str = "scenes";

/// The scene-editor chord's key.
pub const SCENE_EDITOR_KEY: u16 = codes::KEY_P;

/// The scene-editor recovery chord, `Ctrl+Alt+P`: Left or Right Ctrl, each
/// with Left Alt (matching is side-exact, so one row per state). Right Alt
/// is left out: it is AltGr on some layouts, so `Ctrl+AltGr+P` may be a
/// character someone types.
pub const SCENE_EDITOR_CHORDS: [SideModifiers; 2] = [
    SideModifiers {
        left_ctrl: true,
        left_alt: true,
        ..SideModifiers::NONE
    },
    SideModifiers {
        right_ctrl: true,
        left_alt: true,
        ..SideModifiers::NONE
    },
];

/// The shipped keymap. It binds **no user key**: every bare F-key and every
/// right-hand arrow is unbound, so the classifier swallows it until the
/// person binds it. The only rows are the system-reserved scene-editor
/// recovery chord, `scenes.editor.open {"safe":true}` sent to the loader,
/// which treats a safe open over a visible editor as a close.
pub fn default_keymap() -> InputKeymap {
    InputKeymap {
        version: inputd_schema::KEYMAP_SCHEMA_VERSION,
        physical: SCENE_EDITOR_CHORDS
            .into_iter()
            .map(|modifiers| PhysicalBinding {
                stroke: PhysicalStroke {
                    code: SCENE_EDITOR_KEY,
                    modifiers,
                },
                action: ActionId::from_static("scenes.editor.open"),
                args: Some(serde_json::json!({"safe": true})),
                service: Some(SCENES_SERVICE.to_string()),
                scope: BindingScope::default(),
                // A held chord must not flap the toggle.
                repeat: RepeatPolicy::Ignore,
                passthrough: false,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
