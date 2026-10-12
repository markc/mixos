// SPDX-License-Identifier: MIT OR Apache-2.0
//! Key presses, text and wheel travel into terminal actions and PTY keys.
//!
//! Pure functions over egui's input types, so the mapping is testable
//! without a compositor, and every case is pinned against the encoder the
//! PTY actually hears (`term_core::terminal::encode`).
//!
//! What egui hands over, and what follows from it:
//!
//! - A key press is an [`egui::Key`]: the layout's key when egui knows it,
//!   else the key at that position on a US layout. So on a Cyrillic layout
//!   Ctrl+Shift+T still opens a tab, and on Dvorak the key labelled T does.
//! - Text arrives separately, as typed (any layout, any script), and never
//!   while Ctrl is held: Ctrl is a chord, never text.
//! - Ctrl+C, Ctrl+X and Ctrl+V (with or without Shift) arrive as copy, cut
//!   and paste events instead of key presses; [`clipboard_event`] turns them
//!   back into the terminal's chords and control codes.
//! - Left and right modifiers look the same. Right-hand modifier chords are
//!   the person's own keys on MixOS, so nothing here needs them.

use egui::{Key, Modifiers, MouseWheelUnit};
use term_core::panes::{Direction, SplitDir};
use term_core::terminal::{Key as TerminalKey, ScrollRequest};

/// What a chord does to the terminal rather than to the shell in it.
///
/// Tabs and panes keep the chords the terminal has always had; the font
/// chords are foot's (`man 5 foot.ini` § key-bindings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    CloseTab,
    Quit,
    Split(SplitDir),
    ClosePane,
    Focus(Direction),
    Cycle { forward: bool },
    CyclePane { forward: bool },
    FontIncrease,
    FontDecrease,
    FontReset,
    Scroll(ScrollRequest),
    Copy,
    Paste,
}

impl Action {
    /// Whether holding the chord repeats it. Font steps and scrolling do, as
    /// in foot; a held Ctrl+Shift+T must not open a tab per autorepeat tick.
    /// A repeat of a non-repeating chord is still CONSUMED: it never reaches
    /// the PTY as a control code.
    pub fn repeats(self) -> bool {
        matches!(
            self,
            Self::FontIncrease | Self::FontDecrease | Self::FontReset | Self::Scroll(_)
        )
    }

    /// The command that runs this action (`commands.rs`).
    pub fn command(self) -> &'static str {
        match self {
            Self::NewTab => "tab.new",
            Self::CloseTab => "tab.close",
            Self::Quit => "app.quit",
            Self::Split(SplitDir::Vertical) => "pane.split_right",
            Self::Split(SplitDir::Horizontal) => "pane.split_down",
            Self::ClosePane => "pane.close",
            Self::Focus(Direction::Left) => "pane.focus_left",
            Self::Focus(Direction::Right) => "pane.focus_right",
            Self::Focus(Direction::Up) => "pane.focus_up",
            Self::Focus(Direction::Down) => "pane.focus_down",
            Self::Cycle { forward: true } => "tab.next",
            Self::Cycle { forward: false } => "tab.prev",
            Self::CyclePane { forward: true } => "pane.next",
            Self::CyclePane { forward: false } => "pane.prev",
            Self::FontIncrease => "view.zoom_in",
            Self::FontDecrease => "view.zoom_out",
            Self::FontReset => "view.zoom_reset",
            Self::Scroll(ScrollRequest::PageUp) => "scroll.page_up",
            Self::Scroll(ScrollRequest::PageDown) => "scroll.page_down",
            Self::Scroll(ScrollRequest::Top) => "scroll.top",
            Self::Scroll(_) => "scroll.bottom",
            Self::Copy => "edit.copy",
            Self::Paste => "edit.paste",
        }
    }
}

/// The terminal's own chord for a key press, if it is one.
///
/// Alt chords are never ours, exactly as in [`keys_for`].
pub fn action_for(key: Key, modifiers: Modifiers) -> Option<Action> {
    if modifiers.alt {
        return None;
    }
    if !modifiers.ctrl {
        return if modifiers.shift {
            if key == Key::Insert {
                Some(Action::Paste)
            } else {
                scroll_request(key).map(Action::Scroll)
            }
        } else {
            None
        };
    }
    let shift = modifiers.shift;
    if key == Key::Tab {
        return Some(Action::CyclePane { forward: !shift });
    }
    // Control+plus is Shift+= on a US layout, which egui reports as plus;
    // a layout with its own plus key and the keypad's plus report plus too.
    if matches!(key, Key::Plus | Key::Equals) {
        return Some(Action::FontIncrease);
    }
    // Shift is refused for minus and zero: Shift+0 is ")" on US, and a
    // layout egui cannot name falls back to the key's US position.
    if !shift && key == Key::Minus {
        return Some(Action::FontDecrease);
    }
    if !shift && key == Key::Num0 {
        return Some(Action::FontReset);
    }
    if shift && let Some(letter) = letter(key) {
        return Some(match letter {
            'c' => Action::Copy,
            'v' => Action::Paste,
            't' => Action::NewTab,
            'w' => Action::CloseTab,
            'q' => Action::Quit,
            'e' => Action::Split(SplitDir::Vertical),
            'o' => Action::Split(SplitDir::Horizontal),
            'x' => Action::ClosePane,
            _ => return None,
        });
    }
    match key {
        Key::ArrowLeft if shift => Some(Action::Focus(Direction::Left)),
        Key::ArrowRight if shift => Some(Action::Focus(Direction::Right)),
        Key::ArrowUp if shift => Some(Action::Focus(Direction::Up)),
        Key::ArrowDown if shift => Some(Action::Focus(Direction::Down)),
        Key::PageDown if !shift => Some(Action::Cycle { forward: true }),
        Key::PageUp if !shift => Some(Action::Cycle { forward: false }),
        _ => None,
    }
}

fn scroll_request(key: Key) -> Option<ScrollRequest> {
    Some(match key {
        Key::PageUp => ScrollRequest::PageUp,
        Key::PageDown => ScrollRequest::PageDown,
        Key::Home => ScrollRequest::Top,
        Key::End => ScrollRequest::Bottom,
        _ => return None,
    })
}

/// Alternate-screen applications own the scrollback chords too.
pub fn action_on_screen(action: Option<Action>, alternate: bool) -> Option<Action> {
    action.filter(|action| !alternate || !matches!(action, Action::Scroll(_)))
}

/// What a clipboard event from the platform is. egui makes Ctrl+C, Ctrl+X
/// and Ctrl+V into copy, cut and paste events whether or not Shift is held,
/// so Shift decides: with it they are the terminal's chords, without it
/// the shell's control codes (ESC-prefixed with Alt: Ctrl+Alt+C is ESC ^C).
#[derive(Clone, Debug)]
pub enum Clipboard {
    Action(Action),
    /// Paste this text (Ctrl+Shift+V: the platform has read the clipboard).
    Paste(String),
    Keys(Vec<TerminalKey>),
}

/// The meaning of `event` (Copy, Cut or Paste) under `modifiers`.
pub fn clipboard_event(event: &egui::Event, modifiers: Modifiers) -> Option<Clipboard> {
    let letter = match event {
        egui::Event::Copy => 'c',
        egui::Event::Cut => 'x',
        egui::Event::Paste(_) => 'v',
        _ => return None,
    };
    if modifiers.alt {
        return Some(Clipboard::Keys(meta(vec![TerminalKey::Control(letter)])));
    }
    Some(match event {
        _ if !modifiers.shift => Clipboard::Keys(vec![TerminalKey::Control(letter)]),
        egui::Event::Copy => Clipboard::Action(Action::Copy),
        egui::Event::Cut => Clipboard::Action(Action::ClosePane),
        egui::Event::Paste(text) => Clipboard::Paste(text.clone()),
        _ => return None,
    })
}

/// Alt as Meta: ESC before the keys (readline's Alt+b, Alt+., Alt+Backspace,
/// a TUI's Alt+Enter).
pub fn meta(keys: Vec<TerminalKey>) -> Vec<TerminalKey> {
    if keys.is_empty() {
        return keys;
    }
    let mut out = Vec::with_capacity(keys.len() + 1);
    out.push(TerminalKey::Escape);
    out.extend(keys);
    out
}

/// Layout order is the pane tree's first-child/second-child traversal.
pub fn cycle_pane(ids: &[u64], active: u64, forward: bool) -> Option<u64> {
    let current = ids.iter().position(|id| *id == active)?;
    let next = if forward {
        (current + 1) % ids.len()
    } else {
        (current + ids.len() - 1) % ids.len()
    };
    Some(ids[next])
}

/// The cell under a point `position` relative to a pane's outer border, in
/// points. The border and the spare right and bottom pixels clamp to the
/// nearest cell.
pub fn pointer_cell(
    position: egui::Pos2,
    border: f32,
    cell: (f32, f32),
    grid: (u16, u16),
) -> (u16, u16) {
    let at = |offset: f32, size: f32, count: u16| {
        let index = ((offset - border) / size.max(f32::EPSILON)).floor();
        if index.is_finite() {
            index.clamp(0.0, f32::from(count.max(1) - 1)) as u16
        } else {
            0
        }
    };
    (
        at(position.x, cell.0, grid.0),
        at(position.y, cell.1, grid.1),
    )
}

/// Points of smooth (touchpad) scrolling that make one zoom step. A
/// notched wheel reports whole lines and steps once per notch.
pub const PIXELS_PER_STEP: f32 = 40.0;

/// How far one wheel event moves, in lines (`Line` and `Page` units) or
/// points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wheel {
    Lines(f32),
    Points(f32),
}

impl Wheel {
    /// The vertical travel of an egui wheel event; positive is away from
    /// the person (up).
    pub fn of(unit: MouseWheelUnit, delta: egui::Vec2) -> Self {
        match unit {
            MouseWheelUnit::Point => Self::Points(delta.y),
            MouseWheelUnit::Line | MouseWheelUnit::Page => Self::Lines(delta.y),
        }
    }
}

/// Ctrl+wheel into font steps: positive grows, negative shrinks. Positive
/// is wheel-up, the direction foot calls `BTN_WHEEL_BACK`, and foot binds
/// `font-increase=Control+BTN_WHEEL_BACK`, so the two agree.
///
/// Fractions accumulate in `pending`, so a touchpad's stream of small
/// deltas adds up to steps instead of rounding each to zero; turning round
/// discards what was accumulated the other way.
pub fn wheel_steps(pending: &mut f32, wheel: Wheel) -> i32 {
    scroll_steps(pending, wheel, PIXELS_PER_STEP)
}

/// Scrollback travel: touchpad points count in cell heights.
pub fn scroll_steps(pending: &mut f32, wheel: Wheel, cell_height: f32) -> i32 {
    let amount = match wheel {
        Wheel::Lines(y) => y,
        Wheel::Points(y) => y / cell_height,
    };
    if !amount.is_finite() || amount == 0.0 {
        return 0;
    }
    if amount.signum() != pending.signum() {
        *pending = 0.0;
    }
    *pending += amount;
    let steps = pending.trunc();
    *pending -= steps;
    steps as i32
}

/// The PTY keys one key press sends. Empty means the press types text,
/// which arrives as its own event ([`text_keys`], ESC-prefixed under Alt).
///
/// Alt is Meta, as in xterm with `metaSendsEscape`: ESC before the key's
/// own bytes (Alt+Enter is ESC CR, Alt+Backspace ESC DEL, Ctrl+Alt+x
/// ESC ^X), except the cursor keys, which carry the xterm modifier
/// (Alt+Left is CSI 1;3D).
pub fn keys_for(key: Key, modifiers: Modifiers) -> Vec<TerminalKey> {
    if modifiers.alt {
        if let Some(code) = cursor_code(key) {
            return vec![TerminalKey::Modified { code, modifier: 3 }];
        }
        let plain = Modifiers {
            alt: false,
            ..modifiers
        };
        return meta(keys_for(key, plain));
    }
    if modifiers.ctrl {
        // Ctrl is a chord, never text: the control code comes from the
        // letter or punctuation key, whatever text the seat would have
        // reported (egui reports none under Ctrl).
        return letter(key)
            .or_else(|| control_punctuation(key, modifiers.shift))
            .map(TerminalKey::Control)
            .into_iter()
            .collect();
    }
    named_key(key).into_iter().collect()
}

/// The character whose control code a Ctrl+punctuation key sends, as
/// xterm and foot do: Ctrl+[ is ESC (Vim's escape), Ctrl+\ FS, Ctrl+] GS,
/// Ctrl+Space and Ctrl+@ (Shift+2) NUL, Ctrl+^ (Shift+6) RS, Ctrl+_
/// (Shift+minus) and Ctrl+/ US. Shifted keys arrive as their US position
/// when egui has no name for the shifted character.
fn control_punctuation(key: Key, shift: bool) -> Option<char> {
    Some(match key {
        Key::OpenBracket => '[',
        Key::Backslash => '\\',
        Key::CloseBracket => ']',
        Key::Space => ' ',
        Key::Slash => '_',
        Key::Num2 if shift => '@',
        Key::Num6 if shift => '^',
        Key::Minus if shift => '_',
        _ => return None,
    })
}

/// The final byte of a cursor key's xterm sequence.
fn cursor_code(key: Key) -> Option<u8> {
    Some(match key {
        Key::ArrowUp => b'A',
        Key::ArrowDown => b'B',
        Key::ArrowRight => b'C',
        Key::ArrowLeft => b'D',
        Key::Home => b'H',
        Key::End => b'F',
        _ => return None,
    })
}

/// Typed text into PTY keys. Shared by key text and IME commits; format
/// characters (ZWJ, variation selectors) are text, controls are not.
pub fn text_keys(text: &str) -> Vec<TerminalKey> {
    text.chars()
        .filter(|c| !c.is_control())
        .map(TerminalKey::Char)
        .collect()
}

/// The keys typed text sends under `modifiers`: itself, ESC-prefixed
/// under Alt (egui reports the text of Alt chords, the layout's character),
/// nothing under Ctrl (the key press carries the chord).
pub fn typed(text: &str, modifiers: Modifiers) -> Vec<TerminalKey> {
    if modifiers.ctrl {
        return Vec::new();
    }
    let keys = text_keys(text);
    if modifiers.alt { meta(keys) } else { keys }
}

/// The lowercase ASCII letter a letter key stands for.
fn letter(key: Key) -> Option<char> {
    let mut chars = key.name().chars();
    let c = chars.next()?;
    (c.is_ascii_alphabetic() && chars.next().is_none()).then(|| c.to_ascii_lowercase())
}

fn named_key(key: Key) -> Option<TerminalKey> {
    Some(match key {
        Key::Enter => TerminalKey::Enter,
        Key::Backspace => TerminalKey::Backspace,
        Key::Tab => TerminalKey::Tab,
        Key::Escape => TerminalKey::Escape,
        Key::ArrowUp => TerminalKey::Up,
        Key::ArrowDown => TerminalKey::Down,
        Key::ArrowLeft => TerminalKey::Left,
        Key::ArrowRight => TerminalKey::Right,
        Key::Home => TerminalKey::Home,
        Key::End => TerminalKey::End,
        Key::Delete => TerminalKey::Delete,
        Key::PageUp => TerminalKey::PageUp,
        Key::PageDown => TerminalKey::PageDown,
        // Space carries its own text; anything else here is not a VT key.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::terminal::encode;

    const CTRL: Modifiers = Modifiers::CTRL;
    const SHIFT: Modifiers = Modifiers::SHIFT;
    const ALT: Modifiers = Modifiers::ALT;
    const NONE: Modifiers = Modifiers::NONE;

    fn ctrl_shift() -> Modifiers {
        CTRL | SHIFT
    }

    /// Asserts on the BYTES, not on the enum: the bytes are what the shell
    /// actually receives.
    fn bytes(key: Key, modifiers: Modifiers) -> Vec<u8> {
        keys_for(key, modifiers)
            .into_iter()
            .flat_map(encode)
            .collect()
    }

    fn text(text: &str) -> Vec<u8> {
        text_keys(text).into_iter().flat_map(encode).collect()
    }

    #[test]
    fn clipboard_events_are_chords_with_shift_and_control_codes_without() {
        let codes = |event: &egui::Event| match clipboard_event(event, CTRL) {
            Some(Clipboard::Keys(keys)) => keys.into_iter().flat_map(encode).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(codes(&egui::Event::Copy), [3]);
        assert_eq!(codes(&egui::Event::Cut), [24]);
        assert_eq!(codes(&egui::Event::Paste("x".into())), [22]);
        assert!(matches!(
            clipboard_event(&egui::Event::Copy, ctrl_shift()),
            Some(Clipboard::Action(Action::Copy))
        ));
        assert!(matches!(
            clipboard_event(&egui::Event::Cut, ctrl_shift()),
            Some(Clipboard::Action(Action::ClosePane))
        ));
        assert!(matches!(
            clipboard_event(&egui::Event::Paste("hi".into()), ctrl_shift()),
            Some(Clipboard::Paste(text)) if text == "hi"
        ));
        // Ctrl+Alt+C is Meta+^C.
        assert!(matches!(
            clipboard_event(&egui::Event::Copy, CTRL | ALT),
            Some(Clipboard::Keys(keys)) if keys.iter().copied().flat_map(encode).collect::<Vec<_>>() == [0x1b, 3]
        ));
        // A driven Ctrl+Shift+C arrives as a key press: the same chord.
        assert_eq!(action_for(Key::C, ctrl_shift()), Some(Action::Copy));
        assert_eq!(action_for(Key::V, ctrl_shift()), Some(Action::Paste));
        assert_eq!(action_for(Key::Insert, SHIFT), Some(Action::Paste));
        for modifiers in [NONE, CTRL, ctrl_shift(), SHIFT | ALT] {
            assert_eq!(action_for(Key::Insert, modifiers), None);
        }
        assert_eq!(action_for(Key::C, CTRL), None);
        assert_eq!(bytes(Key::C, CTRL), [3]);
        assert!(!Action::Copy.repeats() && !Action::Paste.repeats());
        assert_eq!(
            action_on_screen(Some(Action::Copy), true),
            Some(Action::Copy)
        );
    }

    #[test]
    fn plain_text_reaches_the_pty_as_itself() {
        assert_eq!(text("a"), b"a");
        assert_eq!(text(" "), b" ");
        assert_eq!(text("A"), b"A");
        // A letter key types through its text, not its key press.
        assert!(bytes(Key::A, NONE).is_empty());
        assert!(bytes(Key::Space, NONE).is_empty());
        let typed_bytes = |t: &str, m| typed(t, m).into_iter().flat_map(encode).collect::<Vec<_>>();
        assert_eq!(typed_bytes("A", SHIFT), b"A");
        assert!(
            typed_bytes("a", CTRL).is_empty(),
            "Ctrl is a chord, never text"
        );
    }

    /// GLM review: Alt chords were dropped. Alt is Meta: ESC, then the
    /// key's own bytes; cursor keys carry the xterm modifier.
    #[test]
    fn alt_is_meta() {
        let typed_bytes = |t: &str, m| typed(t, m).into_iter().flat_map(encode).collect::<Vec<_>>();
        // readline: Alt+b, Alt+f, Alt+. arrive as text with Alt held.
        assert_eq!(typed_bytes("b", ALT), b"\x1bb");
        assert_eq!(typed_bytes("f", ALT), b"\x1bf");
        assert_eq!(typed_bytes(".", ALT), b"\x1b.");
        assert_eq!(typed_bytes("B", ALT | SHIFT), b"\x1bB");
        // Their key presses add nothing: the text carries them.
        assert!(bytes(Key::B, ALT).is_empty());
        assert!(bytes(Key::Space, ALT).is_empty());
        assert_eq!(bytes(Key::Backspace, ALT), b"\x1b\x7f");
        assert_eq!(bytes(Key::Enter, ALT), b"\x1b\r");
        assert_eq!(bytes(Key::Escape, ALT), b"\x1b\x1b");
        assert_eq!(bytes(Key::Tab, ALT), b"\x1b\t");
        assert_eq!(bytes(Key::Delete, ALT), b"\x1b\x1b[3~");
        assert_eq!(bytes(Key::X, CTRL | ALT), b"\x1b\x18");
        assert_eq!(bytes(Key::OpenBracket, CTRL | ALT), b"\x1b\x1b");
        for (key, sequence) in [
            (Key::ArrowUp, b"\x1b[1;3A"),
            (Key::ArrowDown, b"\x1b[1;3B"),
            (Key::ArrowRight, b"\x1b[1;3C"),
            (Key::ArrowLeft, b"\x1b[1;3D"),
            (Key::Home, b"\x1b[1;3H"),
            (Key::End, b"\x1b[1;3F"),
        ] {
            assert_eq!(bytes(key, ALT), sequence, "{key:?}");
        }
        // Alt chords are never the terminal's own.
        assert_eq!(action_for(Key::T, ctrl_shift() | ALT), None);
        assert_eq!(action_for(Key::Equals, CTRL | ALT), None);
    }

    #[test]
    fn named_keys_send_their_sequences() {
        assert_eq!(bytes(Key::Enter, NONE), b"\r");
        assert_eq!(bytes(Key::Tab, NONE), b"\t");
        assert_eq!(bytes(Key::ArrowUp, NONE), b"\x1b[A");
        assert_eq!(bytes(Key::PageDown, NONE), b"\x1b[6~");
        assert_eq!(bytes(Key::Backspace, NONE), [127]);
        // Text the seat reports for Enter or Tab is a control and dropped,
        // so one Enter reaches the PTY, never two.
        assert!(text("\r").is_empty());
        assert!(text("\t").is_empty());
    }

    #[test]
    fn control_chords_come_from_the_letter() {
        assert_eq!(bytes(Key::C, CTRL), [3]);
        assert_eq!(bytes(Key::D, CTRL), [4]);
        assert_eq!(bytes(Key::I, CTRL), b"\t");
    }

    #[test]
    fn control_punctuation_sends_its_control_codes() {
        let cs = ctrl_shift();
        for (key, modifiers, byte) in [
            (Key::OpenBracket, CTRL, 0x1b),
            (Key::Backslash, CTRL, 0x1c),
            (Key::CloseBracket, CTRL, 0x1d),
            (Key::Space, CTRL, 0x00),
            (Key::Num2, cs, 0x00),
            (Key::Num6, cs, 0x1e),
            (Key::Minus, cs, 0x1f),
            (Key::Slash, CTRL, 0x1f),
        ] {
            assert_eq!(action_for(key, modifiers), None, "{key:?}");
            assert_eq!(bytes(key, modifiers), [byte], "{key:?}");
        }
        // Unshifted digits and minus stay out of it (Ctrl+- is a font step).
        assert!(bytes(Key::Num2, CTRL).is_empty());
        assert!(bytes(Key::Num6, CTRL).is_empty());
        assert_eq!(action_for(Key::Minus, CTRL), Some(Action::FontDecrease));
    }

    #[test]
    fn chords_this_frontend_does_not_own_are_left_alone() {
        assert!(bytes(Key::F5, CTRL).is_empty());
        assert!(bytes(Key::Num1, CTRL).is_empty());
        assert_eq!(action_for(Key::T, ctrl_shift() | ALT), None);
    }

    #[test]
    fn tab_and_pane_chords() {
        let cases = [
            (Key::T, Action::NewTab),
            (Key::W, Action::CloseTab),
            (Key::Q, Action::Quit),
            (Key::E, Action::Split(SplitDir::Vertical)),
            (Key::O, Action::Split(SplitDir::Horizontal)),
            (Key::X, Action::ClosePane),
        ];
        for (key, action) in cases {
            assert_eq!(action_for(key, ctrl_shift()), Some(action));
            // Without Shift it is the shell's Ctrl+letter, not ours.
            assert_eq!(action_for(key, CTRL), None, "{key:?}");
        }
        for (arrow, direction) in [
            (Key::ArrowLeft, Direction::Left),
            (Key::ArrowRight, Direction::Right),
            (Key::ArrowUp, Direction::Up),
            (Key::ArrowDown, Direction::Down),
        ] {
            assert_eq!(
                action_for(arrow, ctrl_shift()),
                Some(Action::Focus(direction))
            );
            assert_eq!(action_for(arrow, CTRL), None);
            // Shift+arrow is the shell's (selection in an editor).
            assert_eq!(action_for(arrow, SHIFT), None);
        }
        assert_eq!(
            action_for(Key::PageDown, CTRL),
            Some(Action::Cycle { forward: true })
        );
        assert_eq!(
            action_for(Key::PageUp, CTRL),
            Some(Action::Cycle { forward: false })
        );
        // Tabs cycle on Ctrl+PageUp/PageDown WITHOUT Shift only.
        assert_eq!(action_for(Key::PageDown, ctrl_shift()), None);
        assert!(!Action::NewTab.repeats());
        assert!(!Action::ClosePane.repeats());
    }

    /// The chord must beat the shell encoder: without the action table,
    /// Ctrl+Shift+T is Ctrl-T on the PTY. This pins that the two tables
    /// overlap, so the dispatcher's "action first" order is load-bearing.
    #[test]
    fn a_tab_chord_would_otherwise_reach_the_shell_as_a_control_code() {
        assert_eq!(bytes(Key::T, ctrl_shift()), [20]);
        assert!(action_for(Key::T, ctrl_shift()).is_some());
    }

    /// foot's font-increase / font-decrease / font-reset bindings.
    #[test]
    fn font_chords_match_foot() {
        assert_eq!(action_for(Key::Equals, CTRL), Some(Action::FontIncrease));
        // Control+plus on US is Shift+=, reported as plus.
        assert_eq!(
            action_for(Key::Plus, ctrl_shift()),
            Some(Action::FontIncrease)
        );
        assert_eq!(action_for(Key::Plus, CTRL), Some(Action::FontIncrease));
        assert_eq!(action_for(Key::Minus, CTRL), Some(Action::FontDecrease));
        assert_eq!(action_for(Key::Num0, CTRL), Some(Action::FontReset));
        // Ctrl+Shift+0 (")" on US) and Ctrl+Shift+minus are not ours.
        assert_eq!(action_for(Key::Num0, ctrl_shift()), None);
        assert_eq!(action_for(Key::Minus, ctrl_shift()), None);
        // Without Ctrl, or with Alt, these are text.
        assert_eq!(action_for(Key::Equals, NONE), None);
        assert_eq!(action_for(Key::Minus, CTRL | ALT), None);
        assert!(Action::FontIncrease.repeats());
        assert!(Action::FontDecrease.repeats());
    }

    #[test]
    fn ctrl_tab_cycles_panes_but_bare_tab_stays_with_the_shell() {
        assert_eq!(
            action_for(Key::Tab, CTRL),
            Some(Action::CyclePane { forward: true })
        );
        assert_eq!(
            action_for(Key::Tab, ctrl_shift()),
            Some(Action::CyclePane { forward: false })
        );
        assert_eq!(action_for(Key::Tab, NONE), None);
        assert_eq!(action_for(Key::Tab, CTRL | ALT), None);
        assert!(!Action::CyclePane { forward: true }.repeats());
        let ids = [9, 3, 17]; // Layout order, deliberately not numeric order.
        assert_eq!(cycle_pane(&ids, 9, true), Some(3));
        assert_eq!(cycle_pane(&ids, 17, true), Some(9));
        assert_eq!(cycle_pane(&ids, 9, false), Some(17));
        assert_eq!(cycle_pane(&ids, 17, false), Some(3));
        assert_eq!(cycle_pane(&[9], 9, false), Some(9));
        assert_eq!(cycle_pane(&[], 9, true), None);
    }

    #[test]
    fn shift_navigation_scrolls_plain_navigation_is_shell_input() {
        for (key, request, shell) in [
            (Key::PageUp, ScrollRequest::PageUp, b"\x1b[5~".as_slice()),
            (
                Key::PageDown,
                ScrollRequest::PageDown,
                b"\x1b[6~".as_slice(),
            ),
            (Key::Home, ScrollRequest::Top, b"\x1b[H".as_slice()),
            (Key::End, ScrollRequest::Bottom, b"\x1b[F".as_slice()),
        ] {
            assert_eq!(action_for(key, SHIFT), Some(Action::Scroll(request)));
            assert!(Action::Scroll(request).repeats());
            assert_eq!(action_for(key, NONE), None);
            assert_eq!(bytes(key, NONE), shell);
            assert_eq!(action_for(key, SHIFT | ALT), None);
            // The alternate screen owns them: the shell gets the key.
            assert_eq!(action_on_screen(action_for(key, SHIFT), true), None);
            assert_eq!(bytes(key, SHIFT), shell);
        }
    }

    #[test]
    fn pointer_coordinates_exclude_the_border_and_clamp_spare_pixels() {
        let cell = (8.0, 16.0);
        let at = |x, y| pointer_cell(egui::pos2(x, y), 1.2, cell, (80, 24));
        assert_eq!(at(1.2, 1.2), (0, 0));
        assert_eq!(at(25.3, 33.3), (3, 2));
        assert_eq!(at(0.0, 0.0), (0, 0));
        assert_eq!(at(900.0, 500.0), (79, 23));
        assert_eq!(at(f32::NAN, 2.0), (0, 0));
    }

    #[test]
    fn touchpad_history_uses_cell_height_while_zoom_keeps_forty_points() {
        for height in [13.0, 17.0, 24.0] {
            let mut pending = 0.0;
            let half = Wheel::Points(height / 2.0);
            assert_eq!(scroll_steps(&mut pending, half, height), 0);
            assert_eq!(scroll_steps(&mut pending, half, height), 1);
            assert_eq!(
                scroll_steps(&mut pending, Wheel::Points(-height), height),
                -1
            );
        }
        assert_eq!(wheel_steps(&mut 0.0, Wheel::Points(40.0)), 1);
        assert_eq!(
            Wheel::of(MouseWheelUnit::Line, egui::vec2(0.0, 2.0)),
            Wheel::Lines(2.0)
        );
        assert_eq!(
            Wheel::of(MouseWheelUnit::Point, egui::vec2(3.0, -4.0)),
            Wheel::Points(-4.0)
        );
    }

    #[test]
    fn a_notched_wheel_steps_once_per_notch_in_its_direction() {
        let mut pending = 0.0;
        assert_eq!(wheel_steps(&mut pending, Wheel::Lines(1.0)), 1);
        assert_eq!(wheel_steps(&mut pending, Wheel::Lines(3.0)), 3);
        assert_eq!(wheel_steps(&mut pending, Wheel::Lines(-1.0)), -1);
        assert_eq!(wheel_steps(&mut pending, Wheel::Lines(0.0)), 0);
        assert_eq!(wheel_steps(&mut pending, Wheel::Lines(f32::NAN)), 0);
    }

    #[test]
    fn smooth_scrolling_accumulates_and_a_reversal_starts_fresh() {
        let mut pending = 0.0;
        let small = Wheel::Points(PIXELS_PER_STEP / 4.0);
        assert_eq!(wheel_steps(&mut pending, small), 0);
        assert_eq!(wheel_steps(&mut pending, small), 0);
        assert_eq!(wheel_steps(&mut pending, small), 0);
        assert_eq!(wheel_steps(&mut pending, small), 1, "four quarters");
        for _ in 0..3 {
            wheel_steps(&mut pending, small);
        }
        assert_eq!(
            wheel_steps(&mut pending, Wheel::Points(-PIXELS_PER_STEP)),
            -1
        );
    }

    #[test]
    fn keyboard_text_preserves_complete_unicode_sequences() {
        for typed in ["aéb", "👩‍💻", "🇦🇺", "👍🏽", "❤️", "e\u{301}"] {
            assert_eq!(text(typed), typed.as_bytes());
            assert!(term_core::terminal::encode_text(typed).is_err());
        }
        assert_eq!(text("\u{1b}👩‍💻\u{3}"), "👩‍💻".as_bytes());
    }

    #[test]
    fn every_action_names_a_command() {
        let registry = crate::commands::registry();
        for action in [
            Action::NewTab,
            Action::CloseTab,
            Action::Quit,
            Action::Split(SplitDir::Vertical),
            Action::Split(SplitDir::Horizontal),
            Action::ClosePane,
            Action::Focus(Direction::Left),
            Action::Focus(Direction::Right),
            Action::Focus(Direction::Up),
            Action::Focus(Direction::Down),
            Action::Cycle { forward: true },
            Action::Cycle { forward: false },
            Action::CyclePane { forward: true },
            Action::CyclePane { forward: false },
            Action::FontIncrease,
            Action::FontDecrease,
            Action::FontReset,
            Action::Scroll(ScrollRequest::PageUp),
            Action::Scroll(ScrollRequest::PageDown),
            Action::Scroll(ScrollRequest::Top),
            Action::Scroll(ScrollRequest::Bottom),
            Action::Copy,
            Action::Paste,
        ] {
            assert!(
                registry.get(action.command()).is_some(),
                "{action:?} -> {}",
                action.command()
            );
        }
    }
}
