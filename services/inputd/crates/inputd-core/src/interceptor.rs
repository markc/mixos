// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Press-latched key disposition under a grab.
//!
//! Each key is decided **once, at its press**, and the decision is held per
//! key code until that key's release:
//!
//! - **emitted**: the press went to the re-emitter, so its repeats and its
//!   release go there too;
//! - **swallowed**: nothing reaches the re-emitter, and the command latched at
//!   the press (if the row allows repeats) fires again on each repeat.
//!
//! The latch never looks at the modifiers or the keymap again. Releasing
//! Right Ctrl before ← still swallows the ← release, and a rebind while a
//! key is held applies from that key's next press.
//!
//! Modifier keys are tracked for side-exact matching and always emitted, so
//! the compositor's modifier state stays right; they are latched as emitted
//! like any other key.
//!
//! A repeat or release with no latch is an **orphan**: its press happened
//! before this grab began (or before inputd started). It is dropped. The
//! re-emitter never saw that press, so a release there would be a stray.
//!
//! When the grab ends, [`Interceptor::end`] hands back every key latched as
//! emitted, for the caller to release on the re-emitter, and forgets every
//! latch. Swallowed keys need nothing: the re-emitter never saw them.

use std::collections::BTreeMap;

use inputd_schema::{ActionId, SideModifiers};

use crate::{Edge, Resolution, Resolver};

/// A command to fire, with its body and target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fire {
    pub command: ActionId,
    pub args: Option<serde_json::Value>,
    pub service: Option<String>,
}

impl Fire {
    fn from_resolution(resolution: Resolution) -> Option<Self> {
        resolution.command.map(|command| Self {
            command,
            args: resolution.args,
            service: resolution.service,
        })
    }
}

/// What to do with one event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Step {
    /// Write the original event to the re-emitter.
    pub emit: bool,
    /// Fire this command.
    pub fire: Option<Fire>,
    /// The event was a repeat or release with no latch: its press was not
    /// seen. Dropped; the caller may log it.
    pub orphan: bool,
}

impl Step {
    const EMIT: Self = Self {
        emit: true,
        fire: None,
        orphan: false,
    };
    const DROP: Self = Self {
        emit: false,
        fire: None,
        orphan: false,
    };
    const ORPHAN: Self = Self {
        emit: false,
        fire: None,
        orphan: true,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Latch {
    Emitted,
    Swallowed { repeat: Option<Fire> },
}

/// The per-grab key state: side-specific modifiers and the latches.
#[derive(Clone, Debug, Default)]
pub struct Interceptor {
    mods: SideModifiers,
    latches: BTreeMap<u16, Latch>,
}

impl Interceptor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The modifiers held, as this grab has seen them.
    pub fn mods(&self) -> SideModifiers {
        self.mods
    }

    /// Start a grab. Forgets every latch and adopts `held`, the modifiers
    /// physically down at the moment of the grab, so a chord whose modifier
    /// was pressed first still matches. Their presses were not seen under
    /// this grab, so their releases are orphans and are dropped.
    pub fn begin(&mut self, held: SideModifiers) {
        self.latches.clear();
        self.mods = held;
    }

    /// End a grab. Returns, in ascending order, every key code latched as
    /// emitted, which the caller must release on the re-emitter; forgets
    /// every latch and the modifier state.
    pub fn end(&mut self) -> Vec<u16> {
        let emitted = self
            .latches
            .iter()
            .filter(|(_, latch)| **latch == Latch::Emitted)
            .map(|(&code, _)| code)
            .collect();
        self.latches.clear();
        self.mods = SideModifiers::NONE;
        emitted
    }

    /// Recover after the device dropped events (`SYN_DROPPED`): `down` is
    /// every key physically down in the state read once the stream is back
    /// in sync, and `held` the modifiers among them.
    ///
    /// Every key latched as emitted is returned, in ascending order, for
    /// the caller to release on the re-emitter, **whether or not it is
    /// still down**: the physical state cannot show that the press it was
    /// latched for continued, rather than being released and pressed again
    /// inside the lost events. Every latch is forgotten, and each key down
    /// now is quarantined until its release: its repeats and release are
    /// swallowed and nothing fires. The modifiers are rebuilt from `held`.
    pub fn resync(&mut self, down: &[u16], held: SideModifiers) -> Vec<u16> {
        let released = self.end();
        for &code in down {
            self.latches.insert(code, Latch::Swallowed { repeat: None });
        }
        self.mods = held;
        released
    }

    /// True when `code` is held and was decided as emitted.
    pub fn is_emitted(&self, code: u16) -> bool {
        self.latches.get(&code) == Some(&Latch::Emitted)
    }

    /// True when `code` is held and was decided as swallowed.
    pub fn is_swallowed(&self, code: u16) -> bool {
        matches!(self.latches.get(&code), Some(Latch::Swallowed { .. }))
    }

    /// Handle one `EV_KEY` event (`value` 1 press, 2 repeat, 0 release).
    pub fn key(&mut self, resolver: &Resolver, code: u16, value: i32) -> Step {
        if SideModifiers::is_modifier(code) {
            return self.modifier(code, value);
        }
        match value {
            1 => self.press(resolver, code),
            2 => match self.latches.get(&code) {
                Some(Latch::Emitted) => Step::EMIT,
                Some(Latch::Swallowed { repeat }) => Step {
                    fire: repeat.clone(),
                    ..Step::DROP
                },
                None => Step::ORPHAN,
            },
            0 => match self.latches.remove(&code) {
                Some(Latch::Emitted) => Step::EMIT,
                Some(Latch::Swallowed { .. }) => Step::DROP,
                None => Step::ORPHAN,
            },
            _ => Step::DROP,
        }
    }

    fn press(&mut self, resolver: &Resolver, code: u16) -> Step {
        // A second press with no release between (after a dropped-events
        // resync) keeps the first decision: it is treated as a repeat.
        if self.latches.contains_key(&code) {
            return self.key(resolver, code, 2);
        }
        let press = resolver.resolve(code, self.mods, Edge::Press);
        // A quarantined modifier counts for matching (it is physically
        // down) but the apps never saw its press. A key that would go to
        // the apps now would reach them without it (Shift+A as a bare A),
        // so it is quarantined too, until its release.
        if !press.swallow && self.modifier_quarantined() {
            self.latches.insert(code, Latch::Swallowed { repeat: None });
            return Step::DROP;
        }
        if !press.swallow {
            self.latches.insert(code, Latch::Emitted);
            return Step::EMIT;
        }
        let repeat = Fire::from_resolution(resolver.resolve(code, self.mods, Edge::Repeat));
        self.latches.insert(code, Latch::Swallowed { repeat });
        Step {
            fire: Fire::from_resolution(press),
            ..Step::DROP
        }
    }

    /// True while a modifier is held under quarantine. Modifiers are never
    /// swallowed otherwise, so a swallowed modifier latch is one.
    fn modifier_quarantined(&self) -> bool {
        self.latches.iter().any(|(&code, latch)| {
            SideModifiers::is_modifier(code) && matches!(latch, Latch::Swallowed { .. })
        })
    }

    fn modifier(&mut self, code: u16, value: i32) -> Step {
        match value {
            1 => {
                self.mods.set(code, true);
                // A re-press keeps the existing decision (a quarantine too).
                match self.latches.get(&code) {
                    Some(Latch::Swallowed { .. }) => Step::DROP,
                    _ => {
                        self.latches.insert(code, Latch::Emitted);
                        Step::EMIT
                    }
                }
            }
            2 => match self.latches.get(&code) {
                Some(Latch::Emitted) => Step::EMIT,
                Some(Latch::Swallowed { .. }) => Step::DROP,
                None => Step::ORPHAN,
            },
            0 => {
                self.mods.set(code, false);
                match self.latches.remove(&code) {
                    Some(Latch::Emitted) => Step::EMIT,
                    Some(Latch::Swallowed { .. }) => Step::DROP,
                    None => Step::ORPHAN,
                }
            }
            _ => Step::DROP,
        }
    }
}
