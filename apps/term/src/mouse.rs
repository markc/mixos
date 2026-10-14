// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pointer gestures over the panes: selection (drag, double and triple
//! click), mouse reporting to the program in a pane, middle-click paste of
//! the primary selection, and the wheel. Selection itself lives in
//! term-core.
//!
//! Positions travel with each event (window points), never through a
//! coalesced hover position that could already be the next event's.

use crate::app::{App, Effect};
use crate::input::Wheel;
use egui::{PointerButton, Pos2};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use term_core::terminal::{MouseModifiers, SelectionSide, SelectionType, Terminal};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    pane: u64,
    col: u16,
    row: u16,
    side: SelectionSide,
}

#[derive(Clone)]
struct Press {
    pane: u64,
    terminal: Arc<Mutex<Terminal>>,
    last: Hit,
    button: u8,
    local: bool,
    origin: Pos2,
    mods: MouseModifiers,
}

/// One pointer event over the window, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pointer {
    Moved(Pos2),
    Pressed(Pos2, PointerButton),
    Released(Pos2, PointerButton),
}

#[derive(Default)]
pub(crate) struct MouseState {
    pressed: Option<Press>,
    clicks: Option<(Hit, Instant, u8)>,
    /// The cell under the pointer last time it moved.
    hover: Option<Hit>,
}

impl MouseState {
    /// End a cancelled grab exactly once, even after its pane has left the
    /// tab tree. Keep the original terminal alive until the release is sent.
    fn cancel(&mut self) -> bool {
        self.clicks = None;
        let Some(press) = self.pressed.take() else {
            return false;
        };
        let terminal = press.terminal.lock().expect("terminal");
        if !press.local {
            terminal.mouse_button(
                press.last.col,
                press.last.row,
                press.button,
                false,
                press.mods,
            );
        } else if press.button == 0 {
            terminal.selection_clear();
        }
        true
    }

    fn click(&mut self, hit: Hit, at: Instant) -> SelectionType {
        let count = self.clicks.map_or(1, |(last, when, count)| {
            if last.pane == hit.pane
                && last.col == hit.col
                && last.row == hit.row
                && at.saturating_duration_since(when) <= Duration::from_millis(300)
            {
                count % 3 + 1
            } else {
                1
            }
        });
        self.clicks = Some((hit, at, count));
        match count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        }
    }

    /// Whether a button is held over a pane.
    pub(crate) fn grabbed(&self) -> bool {
        self.pressed.is_some()
    }
}

fn button_code(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}

impl App {
    pub fn cancel_mouse_gesture(&mut self) {
        self.mouse.cancel();
    }

    pub(crate) fn cancel_hidden_gesture(&mut self) {
        let hidden = self.mouse.pressed.as_ref().is_some_and(|press| {
            !self
                .tabs
                .lock()
                .expect("tabs")
                .leaves()
                .iter()
                .any(|pane| pane.id == press.pane)
        });
        if hidden {
            self.cancel_mouse_gesture();
        }
    }

    /// The pane and cell at `position`; with `target`, clamped to that pane
    /// even outside its rectangle (an active drag keeps its pane).
    pub(crate) fn hit(&self, position: Pos2, target: Option<u64>) -> Option<Hit> {
        let (pane, rect) = self.placed().into_iter().find(|(id, rect)| {
            target.map_or_else(
                || {
                    position.x >= rect.x
                        && position.x < rect.x + rect.w
                        && position.y >= rect.y
                        && position.y < rect.y + rect.h
                },
                |target| *id == target,
            )
        })?;
        let cell = self.logical_cell();
        let relative = egui::pos2(position.x - rect.x, position.y - rect.y);
        let inset = self.inset();
        let (col, row) =
            crate::input::pointer_cell(relative, inset, cell, *self.grids.get(&pane)?);
        let side = if relative.x - inset - f32::from(col) * cell.0 < cell.0 / 2.0 {
            SelectionSide::Left
        } else {
            SelectionSide::Right
        };
        Some(Hit {
            pane,
            col,
            row,
            side,
        })
    }

    /// The pane under `position`, for the wheel.
    pub fn pane_at(&self, position: Pos2) -> Option<u64> {
        self.hit(position, None).map(|hit| hit.pane)
    }

    /// A wheel event at `position`.
    /// A wheel event at `position`; returns its zoom steps (see
    /// [`App::wheel`]).
    pub fn wheel_at(&mut self, position: Pos2, wheel: Wheel) -> i32 {
        self.pointer = Some(position);
        match self.pane_at(position) {
            Some(pane) => self.wheel(pane, wheel),
            None => 0,
        }
    }

    /// One pointer event. Motion reaches the terminal only when it moves to
    /// another cell (or half cell, for selection), or during a grab.
    pub fn pointer_event(&mut self, event: Pointer, at: Instant) {
        match event {
            Pointer::Moved(position) => {
                self.pointer = Some(position);
                let hit = self.hit(position, None);
                let changed = std::mem::replace(&mut self.mouse.hover, hit) != hit;
                let grabbed = self.mouse.pressed.is_some();
                if !changed && (hit.is_some() || !grabbed) {
                    return;
                }
                if hit.is_none() && !grabbed {
                    return;
                }
                self.mouse_moved(position, false);
            }
            Pointer::Pressed(position, button) => {
                self.pointer = Some(position);
                // The press is where the pointer is: motion from here on
                // is measured from its cell.
                self.mouse.hover = self.hit(position, None);
                self.mouse_pressed(position, button, at);
            }
            Pointer::Released(position, button) => {
                self.pointer = Some(position);
                let Some(code) = button_code(button) else {
                    return;
                };
                if self.mouse.pressed.as_ref().map(|press| press.button) != Some(code) {
                    return;
                }
                self.mouse_moved(position, true);
            }
        }
    }

    fn mouse_pressed(&mut self, position: Pos2, button: PointerButton, at: Instant) {
        let mods = self.mouse_modifiers();
        // A compositor may cancel the grab without a button-up; a new press
        // replaces it, whatever its button.
        if self.mouse.pressed.is_some() {
            self.cancel_mouse_gesture();
        }
        let Some(button) = button_code(button) else {
            return;
        };
        let Some(hit) = self.hit(position, None) else {
            return;
        };
        let mut tabs = self.tabs.lock().expect("tabs");
        let Some(terminal) = tabs.pane_by_id(hit.pane) else {
            return;
        };
        tabs.user_activity();
        // Click-to-focus is the pane.focus command, performed here so the
        // gesture starts on the focused pane; it is published like any run.
        if tabs.active_tab().active_pane != hit.pane && tabs.focus(hit.pane) {
            self.noted.push("pane.focus");
        }
        // Shift overrides the program's mouse reporting.
        let local = mods.shift || !terminal.lock().expect("terminal").mouse_reporting();
        if local && button == 0 {
            // One selection in the window at a time.
            for pane in tabs.control_panes() {
                if pane.id != hit.pane
                    && let Some(other) = tabs.pane_by_id(pane.id)
                {
                    other.lock().expect("terminal").selection_clear();
                }
            }
        }
        drop(tabs);
        self.mouse.pressed = Some(Press {
            pane: hit.pane,
            terminal: terminal.clone(),
            last: hit,
            button,
            local,
            origin: position,
            mods,
        });
        let terminal = terminal.lock().expect("terminal");
        if !local {
            self.mouse.clicks = None;
            terminal.mouse_button(hit.col, hit.row, button, true, mods);
        } else if button == 0 {
            let kind = self.mouse.click(hit, at);
            terminal.selection_start(hit.col, hit.row, hit.side, kind);
        } else if button == 1 {
            self.mouse.clicks = None;
            self.effects.push(Effect::Read {
                pane: hit.pane,
                primary: true,
            });
        }
    }

    /// Motion (or the release, with `released`) during or outside a grab.
    fn mouse_moved(&mut self, position: Pos2, released: bool) {
        let mods = self.mouse_modifiers();
        let press = self.mouse.pressed.clone();
        let Some(hit) = self.hit(position, press.as_ref().map(|p| p.pane)) else {
            self.cancel_mouse_gesture();
            return;
        };
        let tabs = self.tabs.lock().expect("tabs");
        let Some(terminal) = tabs.pane_by_id(hit.pane) else {
            drop(tabs);
            self.cancel_mouse_gesture();
            return;
        };
        drop(tabs);
        if released {
            self.mouse.pressed = None;
        } else if let Some(press) = self.mouse.pressed.as_mut() {
            press.last = hit;
        }
        let terminal = terminal.lock().expect("terminal");
        match press {
            Some(press) if press.local && press.button == 0 => {
                terminal.selection_update(hit.col, hit.row, hit.side);
                if (position.x - press.origin.x).abs() > 5.0
                    || (position.y - press.origin.y).abs() > 5.0
                {
                    self.mouse.clicks = None;
                }
                if released && let Some(text) = terminal.selection_finish() {
                    // terminal.mouse.copy_on_select: the clipboard as well
                    // as the primary selection, as Ctrl+Shift+C would.
                    if self.copy_on_select {
                        self.effects.push(Effect::Write {
                            text: text.clone(),
                            primary: false,
                        });
                    }
                    self.effects.push(Effect::Write {
                        text,
                        primary: true,
                    });
                }
            }
            Some(press) if !press.local => {
                if released {
                    // A matching release even if Shift changed since the
                    // press: local ownership is latched then.
                    terminal.mouse_button(hit.col, hit.row, press.button, false, press.mods);
                } else {
                    terminal.mouse_motion(hit.col, hit.row, press.button, mods);
                }
            }
            Some(_) => {}
            None if !released => {
                terminal.mouse_motion(hit.col, hit.row, 3, mods);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_clicks_require_same_pane_cell_and_time_window() {
        let mut mouse = MouseState::default();
        let hit = Hit {
            pane: 1,
            col: 3,
            row: 2,
            side: SelectionSide::Left,
        };
        let now = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(mouse.click(hit, now), SelectionType::Simple);
        assert_eq!(mouse.click(hit, now + ms(100)), SelectionType::Semantic);
        assert_eq!(mouse.click(hit, now + ms(200)), SelectionType::Lines);
        assert_eq!(mouse.click(hit, now + ms(300)), SelectionType::Simple);
        assert_eq!(mouse.click(hit, now + ms(601)), SelectionType::Simple);
        assert_eq!(
            mouse.click(Hit { pane: 2, ..hit }, now + ms(602)),
            SelectionType::Simple
        );
        assert_eq!(
            mouse.click(
                Hit {
                    pane: 2,
                    col: 4,
                    ..hit
                },
                now + ms(603)
            ),
            SelectionType::Simple
        );
    }
}
