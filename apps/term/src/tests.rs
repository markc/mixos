// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's state on a real PTY-backed tab set, with no window and no
//! Bus: keys, the input method, the pointer, the wheel, zoom, wakes and the
//! chords, each against the bytes a pane's PTY would receive.

use crate::app::{App, Effect, Synced, apply};
use crate::input::{Action, PIXELS_PER_STEP, Wheel};
use crate::layout;
use crate::mouse::Pointer;
use egui::{ImeEvent, Key, Modifiers, PointerButton, Pos2, pos2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use term_core::config::Cursor;
use term_core::font::FontSize;
use term_core::panes::{Direction, Geometry, SplitDir};
use term_core::raster::Raster;
use term_core::tabs::{self, TabSet};
use term_core::terminal::{ScrollRequest, Terminal};

/// The pane area: below a 30-point strip in a 900 x 560 window.
const AREA: Geometry = Geometry {
    x: 0.0,
    y: 30.0,
    w: 900.0,
    h: 530.0,
};

fn ctrl_shift() -> Modifiers {
    Modifiers::CTRL | Modifiers::SHIFT
}

/// A real `App` with a PTY-backed tab set, no window and no Bus.
fn test_app() -> (App, std::thread::JoinHandle<()>) {
    let (cleanup, reaper) = tabs::Cleanup::start().expect("cleanup worker");
    let tabs = Arc::new(Mutex::new(layout::test_tabs()));
    let woken = Arc::new(AtomicBool::new(true));
    let flag = woken.clone();
    tabs.lock()
        .unwrap()
        .set_wake(Arc::new(move || flag.store(true, Ordering::Release)));
    let raster = Raster::for_test(1.0, 13.0, Cursor::Underline).expect("a monospace font");
    let mut app = App::new(tabs, cleanup, None, woken, raster, FontSize::new(13.0));
    assert_eq!(app.sync(), Synced::Running);
    app.layout(AREA);
    (app, reaper)
}

fn finish(app: App, reaper: std::thread::JoinHandle<()>) {
    let removed = app.tabs.lock().unwrap().shutdown();
    app.cleanup.submit(removed);
    drop(app);
    reaper.join().unwrap();
}

/// Replace pane `id`'s PTY with a VT fixture, sized `cols` x `rows`, and
/// return it with a reader of the bytes it is sent.
fn fixture(
    app: &mut App,
    id: u64,
    cols: u16,
    rows: u16,
    bytes: &[u8],
) -> (Arc<Mutex<Terminal>>, Reader) {
    let terminal = app.tabs.lock().unwrap().pane_by_id(id).unwrap();
    *terminal.lock().unwrap() = Terminal::from_test_vt(cols.into(), rows.into(), bytes);
    app.grids.insert(id, (cols, rows));
    let read = Box::new(terminal.lock().unwrap().listener.test_input_reader());
    (terminal, read)
}

/// The bytes a pane's PTY has been sent since the last call.
type Reader = Box<dyn Fn() -> Option<Vec<u8>>>;

fn active_pane(tabs: &TabSet) -> u64 {
    tabs.active_tab().active_pane
}

/// The first cell of the first (or only) pane, a little way in.
fn first_cell(app: &App) -> Pos2 {
    let (cw, ch) = app.logical_cell();
    let border = layout::border(app.scale());
    pos2(border + cw * 0.1, AREA.y + border + ch * 0.5)
}

#[test]
fn chords_come_before_the_shell_and_repeats_are_filtered() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"");
    assert_eq!(app.key(Key::T, ctrl_shift(), false), Some(Action::NewTab));
    // A held Ctrl+Shift+T is one tab: the repeat is swallowed, and NOT
    // handed to the encoder, which would send Ctrl-T to the shell.
    assert_eq!(app.key(Key::T, ctrl_shift(), true), None);
    assert_eq!(read(), None);
    // Font steps repeat when held, as in foot.
    assert_eq!(
        app.key(Key::Equals, Modifiers::CTRL, true),
        Some(Action::FontIncrease)
    );
    // Ctrl+T without Shift is the shell's Ctrl-T.
    assert_eq!(app.key(Key::T, Modifiers::CTRL, false), None);
    assert_eq!(read().as_deref(), Some(&[20][..]));
    // A letter types through its text, repeats included.
    assert_eq!(app.key(Key::A, Modifiers::NONE, true), None);
    assert_eq!(read(), None);
    app.text("a");
    assert_eq!(read().as_deref(), Some(&b"a"[..]));
    // Alt text is not the shell's.
    app.modifiers_changed(Modifiers::ALT);
    app.text("x");
    assert_eq!(read(), None);
    finish(app, reaper);
}

#[test]
fn scrollback_chords_belong_to_the_alternate_screen_when_it_is_up() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"");
    assert_eq!(
        app.key(Key::PageUp, Modifiers::SHIFT, false),
        Some(Action::Scroll(ScrollRequest::PageUp))
    );
    assert_eq!(read(), None);
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"\x1b[?1049h");
    for repeat in [false, true] {
        assert_eq!(app.key(Key::PageUp, Modifiers::SHIFT, repeat), None);
        assert_eq!(read().as_deref(), Some(&b"\x1b[5~"[..]));
    }
    finish(app, reaper);
}

#[test]
fn clipboard_chords_swallow_their_autorepeat_and_control_codes_do_not() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"");
    app.modifiers_changed(ctrl_shift());
    assert_eq!(app.clipboard_event(&egui::Event::Copy), Some(Action::Copy));
    assert_eq!(app.clipboard_event(&egui::Event::Copy), None, "autorepeat");
    app.key_released(Key::C);
    assert_eq!(app.clipboard_event(&egui::Event::Copy), Some(Action::Copy));
    app.key_released(Key::C);
    // Ctrl+Shift+V: the platform read the clipboard; the paste command
    // pastes that text without reading again.
    assert_eq!(
        app.clipboard_event(&egui::Event::Paste("hello".into())),
        Some(Action::Paste)
    );
    app.act(Action::Paste);
    assert_eq!(read().as_deref(), Some(&b"hello"[..]));
    assert!(app.effects.is_empty());
    app.key_released(Key::V);
    // Plain Ctrl+C is the shell's interrupt, every time.
    app.modifiers_changed(Modifiers::CTRL);
    for _ in 0..2 {
        assert_eq!(app.clipboard_event(&egui::Event::Copy), None);
        assert_eq!(read().as_deref(), Some(&[3][..]));
    }
    // A menu paste reads the clipboard for the pane that had the keyboard.
    app.act(Action::Paste);
    assert_eq!(
        app.effects,
        vec![Effect::Read {
            pane: id,
            primary: false
        }]
    );
    finish(app, reaper);
}

#[test]
fn ime_preedit_is_local_and_commit_sends_one_complete_sequence() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"");
    let text = "👩‍💻🇦🇺👍🏽❤️e\u{301}";
    app.ime(&ImeEvent::Preedit {
        text: text.into(),
        active_range_chars: None,
    });
    assert_eq!(app.ime_preedit.as_deref(), Some(text));
    assert_eq!(read(), None);
    app.ime(&ImeEvent::Commit(format!("\u{1b}{text}\u{3}")));
    assert_eq!(read().as_deref(), Some(text.as_bytes()));
    assert_eq!(read(), None);
    assert!(app.ime_preedit.is_none());
    app.unfocused();
    app.ime(&ImeEvent::Commit(text.into()));
    assert_eq!(
        read(),
        None,
        "no input method commits into an unfocused window"
    );
    finish(app, reaper);
}

#[test]
fn ime_focus_switch_and_owner_close_drop_queued_commit_before_wake() {
    for change in ["pane", "tab", "close"] {
        let (mut app, reaper) = test_app();
        let (owner, original_tab, first, second_id) = {
            let mut tabs = app.tabs.lock().unwrap();
            let owner = tabs.active_tab().active_pane;
            let original_tab = tabs.active_id();
            let second_id = tabs.split_active(SplitDir::Vertical).unwrap();
            tabs.focus(owner);
            (owner, original_tab, owner, second_id)
        };
        let (_first, read_first) = fixture(&mut app, first, 8, 3, b"");
        let (_second, _) = fixture(&mut app, second_id, 8, 3, b"");
        app.ime(&ImeEvent::Preedit {
            text: "draft".into(),
            active_range_chars: None,
        });
        // The same mutation routes the Bus uses, with no wake delivered.
        let target = {
            let mut tabs = app.tabs.lock().unwrap();
            match change {
                "pane" => assert!(tabs.focus(second_id)),
                "tab" => {
                    tabs.open().unwrap();
                }
                _ => {
                    let removed = tabs.close_active().1.unwrap();
                    app.cleanup.submit(vec![removed]);
                }
            }
            tabs.active_tab().active_pane
        };
        let (_target, read_target) = if change == "tab" {
            fixture(&mut app, target, 8, 3, b"")
        } else {
            let terminal = app.tabs.lock().unwrap().pane_by_id(target).unwrap();
            let read: Reader = Box::new(terminal.lock().unwrap().listener.test_input_reader());
            (terminal, read)
        };
        app.ime(&ImeEvent::Commit("stale".into()));
        assert_eq!(read_first(), None, "{change}");
        assert_eq!(read_target(), None, "{change}");
        assert!(app.ime_preedit.is_none());
        assert!(!app.ime.stale(), "that commit was the cancelled one");
        // A fresh composition then commits where the keyboard is.
        app.ime(&ImeEvent::Preedit {
            text: "new".into(),
            active_range_chars: None,
        });
        app.ime(&ImeEvent::Commit("new".into()));
        assert_eq!(
            read_target().as_deref(),
            Some(b"new".as_slice()),
            "{change}"
        );
        // Explicit tab selection clears the preedit before its wake too.
        if change == "tab" {
            app.ime(&ImeEvent::Preedit {
                text: "again".into(),
                active_range_chars: None,
            });
            app.select_tab(original_tab);
            assert!(
                app.ime.stale(),
                "the composition in the left tab is cancelled"
            );
            assert!(app.ime_preedit.is_none());
            assert_eq!(active_pane(&app.tabs.lock().unwrap()), owner);
        }
        finish(app, reaper);
    }
}

/// Review finding: compose in pane A, focus pane B, let frames pass, and
/// A's delayed commit must still not reach B.
#[test]
fn a_delayed_commit_after_frames_never_reaches_the_next_pane() {
    let (mut app, reaper) = test_app();
    let a = app.shape.active_pane;
    let b = app
        .tabs
        .lock()
        .unwrap()
        .split_active(SplitDir::Vertical)
        .unwrap();
    app.tabs.lock().unwrap().focus(a);
    let (_a, read_a) = fixture(&mut app, a, 8, 3, b"");
    let (_b, read_b) = fixture(&mut app, b, 8, 3, b"");
    app.ime(&ImeEvent::Preedit {
        text: "draft".into(),
        active_range_chars: None,
    });
    app.tabs.lock().unwrap().focus(b);
    for _ in 0..5 {
        app.sync();
        app.paint();
    }
    assert!(app.ime.take_interrupt(), "the platform is asked to reset");
    app.ime(&ImeEvent::Commit("draft".into()));
    assert_eq!(read_a(), None);
    assert_eq!(read_b(), None, "A's commit landed in B");
    app.ime(&ImeEvent::Commit("b".into()));
    assert_eq!(read_b().as_deref(), Some(&b"b"[..]));
    finish(app, reaper);
}

#[test]
fn tab_uses_live_panes_after_a_tab_switch_without_a_layout_wake() {
    let (mut app, reaper) = test_app();
    app.act(Action::Split(SplitDir::Vertical));
    app.sync();
    assert_eq!(app.shape.visible().len(), 2);
    app.act(Action::NewTab);
    let id = active_pane(&app.tabs.lock().unwrap());
    let (_terminal, read) = fixture(&mut app, id, 8, 3, b"");
    // The shape still describes the split tab, but this tab has one pane.
    for repeat in [false, true] {
        assert_eq!(app.tab_key(true, repeat), None);
        assert_eq!(read().as_deref(), Some(&b"\t"[..]));
    }
    app.sync();
    assert_eq!(app.shape.visible().len(), 1);
    app.act(Action::Cycle { forward: false });
    let ids: Vec<_> = app
        .tabs
        .lock()
        .unwrap()
        .leaves()
        .iter()
        .map(|pane| pane.id)
        .collect();
    assert_eq!(ids.len(), 2);
    let before = active_pane(&app.tabs.lock().unwrap());
    let action = app
        .tab_key(true, false)
        .expect("Tab moves between split panes");
    app.act(action);
    let after = active_pane(&app.tabs.lock().unwrap());
    assert_eq!(Some(after), crate::input::cycle_pane(&ids, before, true));
    assert_ne!(after, before);
    assert_eq!(app.tab_key(true, true), None, "a held Tab moves once");
    let back = app.tab_key(false, false).unwrap();
    app.act(back);
    assert_eq!(active_pane(&app.tabs.lock().unwrap()), before);
    assert!(read().is_none());
    let removed = app.tabs.lock().unwrap().shutdown();
    assert_eq!(app.tab_key(true, false), None);
    app.cleanup.submit(removed);
    finish(app, reaper);
}

#[test]
fn mouse_drag_uses_event_positions_and_release_outside_the_pane() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (terminal, _) = fixture(&mut app, id, 8, 3, b"abcdefgh\r\nijklmnop");
    let (cw, _) = app.logical_cell();
    let start = first_cell(&app);
    let end = pos2(start.x + cw * 3.8, start.y);
    let now = Instant::now();
    app.pointer_event(Pointer::Pressed(start, PointerButton::Primary), now);
    app.pointer_event(Pointer::Moved(end), now);
    assert_eq!(
        terminal.lock().unwrap().selection_text().as_deref(),
        Some("abcd")
    );
    // Moving back to the anchor clears the range again.
    app.pointer_event(Pointer::Moved(start), now);
    assert_eq!(terminal.lock().unwrap().selection_text(), None);
    // An unmatched release must not lose the original grab.
    app.pointer_event(Pointer::Released(end, PointerButton::Secondary), now);
    assert!(app.effects.is_empty());
    let outside = pos2(AREA.w + 20.0, start.y);
    app.pointer_event(Pointer::Released(outside, PointerButton::Primary), now);
    assert_eq!(
        app.effects,
        vec![Effect::Write {
            text: "abcdefgh".into(),
            primary: true
        }],
        "a completed selection writes PRIMARY"
    );
    app.effects.clear();
    app.act(Action::Copy);
    assert_eq!(
        app.effects,
        vec![Effect::Write {
            text: "abcdefgh".into(),
            primary: false
        }]
    );
    app.effects.clear();
    let later = now + Duration::from_secs(1);
    app.pointer_event(Pointer::Pressed(start, PointerButton::Primary), later);
    app.pointer_event(Pointer::Released(start, PointerButton::Primary), later);
    assert!(app.effects.is_empty(), "a click selects nothing");
    assert_eq!(terminal.lock().unwrap().selection_text(), None);
    app.act(Action::Copy);
    assert!(app.effects.is_empty());
    drop(terminal);
    finish(app, reaper);
}

#[test]
fn cancelled_reported_gestures_release_the_original_pane_once() {
    let (mut app, reaper) = test_app();
    let original = app.shape.active_pane;
    let original_tab = app.tabs.lock().unwrap().active_id();
    let (terminal, input) = fixture(&mut app, original, 8, 3, b"\x1b[?1002;1006h");
    let (cw, ch) = app.logical_cell();
    let start = first_cell(&app);
    let end = pos2(start.x + cw * 3.0, start.y + ch);
    let begin = |app: &mut App| {
        app.pointer_event(
            Pointer::Pressed(start, PointerButton::Primary),
            Instant::now(),
        );
        assert_eq!(input().unwrap(), b"\x1b[<0;1;1M");
        app.pointer_event(Pointer::Moved(end), Instant::now());
        assert_eq!(input().unwrap(), b"\x1b[<32;4;2M");
    };
    begin(&mut app);
    app.modifiers_changed(Modifiers::SHIFT);
    app.unfocused();
    assert_eq!(
        input().unwrap(),
        b"\x1b[<0;4;2m",
        "release uses press modifiers and last cell"
    );
    app.pointer_event(
        Pointer::Released(start, PointerButton::Primary),
        Instant::now(),
    );
    assert!(input().is_none(), "late release is not duplicated");

    // Both supported and unknown new buttons terminate the stale grab.
    for button in [
        PointerButton::Primary,
        PointerButton::Secondary,
        PointerButton::Extra1,
    ] {
        begin(&mut app);
        app.pointer_event(Pointer::Pressed(start, button), Instant::now());
        assert_eq!(input().unwrap(), b"\x1b[<0;4;2m");
        if button != PointerButton::Extra1 {
            let code = if button == PointerButton::Primary {
                0
            } else {
                2
            };
            assert_eq!(input().unwrap(), format!("\x1b[<{code};1;1M").as_bytes());
            app.pointer_event(Pointer::Released(start, button), Instant::now());
            assert_eq!(input().unwrap(), format!("\x1b[<{code};1;1m").as_bytes());
        }
        assert!(input().is_none());
    }

    begin(&mut app);
    let tree = app.shape.tree.take();
    app.pointer_event(
        Pointer::Released(start, PointerButton::Primary),
        Instant::now(),
    );
    assert_eq!(
        input().unwrap(),
        b"\x1b[<0;4;2m",
        "missing hit still releases"
    );
    app.shape.tree = tree;

    // A tab switch releases at once, before the next sync.
    let second = app.tabs.lock().unwrap().open().unwrap();
    app.tabs.lock().unwrap().select(original_tab);
    begin(&mut app);
    app.select_tab(second);
    assert_eq!(input().unwrap(), b"\x1b[<0;4;2m");
    app.tabs.lock().unwrap().select(original_tab);

    // External tab mutations reach sync as well.
    begin(&mut app);
    app.tabs.lock().unwrap().select(second);
    app.sync();
    assert_eq!(input().unwrap(), b"\x1b[<0;4;2m");
    app.tabs.lock().unwrap().select(original_tab);
    app.sync();
    begin(&mut app);
    // Removed from the tree before sync: the press keeps its terminal until
    // its release, rather than resolving the now-absent pane id.
    let removed = app.tabs.lock().unwrap().close_active().1.unwrap();
    app.sync();
    assert_eq!(input().unwrap(), b"\x1b[<0;4;2m");
    app.cleanup.submit(vec![removed]);
    app.cancel_mouse_gesture();
    assert!(input().is_none());
    drop(terminal);
    finish(app, reaper);
}

#[test]
fn cancelled_local_drag_clears_and_a_new_pane_selection_is_exclusive() {
    use term_core::terminal::{SelectionSide, SelectionType};
    let (mut app, reaper) = test_app();
    let first = app.shape.active_pane;
    let second = app
        .tabs
        .lock()
        .unwrap()
        .split_active(SplitDir::Vertical)
        .unwrap();
    app.sync();
    let terminals: Vec<_> = [first, second]
        .into_iter()
        .map(|id| fixture(&mut app, id, 8, 3, b"abcdefgh").0)
        .collect();
    terminals[0]
        .lock()
        .unwrap()
        .selection_start(0, 0, SelectionSide::Left, SelectionType::Lines);
    assert!(terminals[0].lock().unwrap().selection_text().is_some());
    let rect = app
        .placed()
        .into_iter()
        .find(|(id, _)| *id == second)
        .unwrap()
        .1;
    let (cw, ch) = app.logical_cell();
    let border = layout::border(app.scale());
    let start = pos2(rect.x + border + cw * 0.1, rect.y + border + ch * 0.5);
    let end = pos2(start.x + cw * 3.8, start.y);
    app.pointer_event(
        Pointer::Pressed(start, PointerButton::Primary),
        Instant::now(),
    );
    assert_eq!(terminals[0].lock().unwrap().selection_text(), None);
    app.pointer_event(Pointer::Moved(end), Instant::now());
    assert_eq!(
        terminals[1].lock().unwrap().selection_text().as_deref(),
        Some("abcd")
    );
    app.unfocused();
    assert_eq!(terminals[1].lock().unwrap().selection_text(), None);
    app.pointer_event(
        Pointer::Released(end, PointerButton::Primary),
        Instant::now(),
    );
    assert!(app.effects.is_empty());
    drop(terminals);
    finish(app, reaper);
}

#[test]
fn oversized_paste_has_a_visible_notice_and_sends_nothing() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (_terminal, input) = fixture(&mut app, id, 8, 3, b"");
    app.paste(id, Some("x".repeat(16 * 1024 * 1024 + 1)));
    assert_eq!(
        app.paste_notice.as_deref(),
        Some("Paste exceeds 16 MiB limit")
    );
    assert!(input().is_none());
    app.paste(id, Some("ok".into()));
    assert_eq!(input().unwrap(), b"ok");
    assert!(app.paste_notice.is_none());
    finish(app, reaper);
}

#[test]
fn mouse_reporting_owns_buttons_unless_shift_started_the_gesture() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    // No PTY sender: a failed report must still NEVER fall back to paste.
    let (terminal, _) = fixture(&mut app, id, 8, 3, b"\x1b[?9hword");
    let position = pos2(4.0, AREA.y + 4.0);
    let now = Instant::now();
    app.pointer_event(Pointer::Pressed(position, PointerButton::Middle), now);
    app.pointer_event(Pointer::Released(position, PointerButton::Middle), now);
    assert!(app.effects.is_empty());
    app.pointer_event(Pointer::Pressed(position, PointerButton::Primary), now);
    app.pointer_event(Pointer::Released(position, PointerButton::Primary), now);
    assert_eq!(terminal.lock().unwrap().selection_text(), None);
    app.modifiers_changed(Modifiers::SHIFT);
    app.pointer_event(Pointer::Pressed(position, PointerButton::Middle), now);
    assert_eq!(
        app.effects,
        vec![Effect::Read {
            pane: id,
            primary: true
        }],
        "Shift+middle pastes the primary selection"
    );
    app.effects.clear();
    app.pointer_event(Pointer::Released(position, PointerButton::Middle), now);
    app.pointer_event(Pointer::Pressed(position, PointerButton::Primary), now);
    // Releasing Shift during this local gesture must not hand it to the app.
    app.modifiers_changed(Modifiers::NONE);
    let end = pos2(position.x + app.logical_cell().0 * 3.8, position.y);
    app.pointer_event(Pointer::Released(end, PointerButton::Primary), now);
    assert!(matches!(
        &app.effects[..],
        [Effect::Write { primary: true, .. }]
    ));
    assert!(terminal.lock().unwrap().selection_text().is_some());
    drop(terminal);
    finish(app, reaper);
}

/// After a zoom, the next paint hands every visible pane a snapshot drawn
/// at its new grid in the new cell: no stretched frame.
#[test]
fn a_zoom_repaints_every_pane_before_the_next_draw() {
    let (mut app, reaper) = test_app();
    app.act(Action::Split(SplitDir::Vertical));
    app.sync();
    assert_eq!(app.shape.visible().len(), 2);
    app.paint();
    let before = app.cell();
    let painted = app.painted.clone();
    app.zoom(|font| font.step_by(6));
    app.paint();
    let cell = app.cell();
    assert_ne!(cell, before, "six steps must change the cell");
    for id in app.shape.visible() {
        let (cols, rows) = app.grids[&id];
        assert_eq!(
            app.targets[&id],
            (u32::from(cols) * cell.0, u32::from(rows) * cell.1),
            "pane {id} still holds the pre-zoom size"
        );
        assert_eq!(app.painted[&id], painted[&id] + 1);
    }
    finish(app, reaper);
}

#[test]
fn wakes_coalesce_at_paint_and_leave_the_clean_neighbour_untouched() {
    let (mut app, reaper) = test_app();
    app.act(Action::Split(SplitDir::Vertical));
    app.sync();
    let ids = app.shape.visible();
    assert_eq!(ids.len(), 2);
    // Let startup output settle, bounded by a deadline.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut quiet = 0;
    while quiet < 5 {
        assert!(Instant::now() < deadline, "PTY startup did not settle");
        let painted = app.painted.clone();
        app.paint();
        std::thread::sleep(Duration::from_millis(10));
        quiet = if app.painted != painted { 0 } else { quiet + 1 };
    }
    let before = [app.painted[&ids[0]], app.painted[&ids[1]]];
    let terminal = app.tabs.lock().unwrap().pane_by_id(ids[0]).unwrap();
    let (cols, rows) = app.grids[&ids[0]];
    terminal.lock().unwrap().resize(cols - 1, rows, 0, 0);
    for _ in 0..20 {
        app.sync();
    }
    assert_eq!(
        [app.painted[&ids[0]], app.painted[&ids[1]]],
        before,
        "wakes must not paint intermediate states"
    );
    app.paint();
    assert_eq!(app.painted[&ids[0]], before[0] + 1);
    assert_eq!(app.painted[&ids[1]], before[1], "clean neighbour repainted");
    app.paint();
    assert_eq!(app.painted[&ids[0]], before[0] + 1, "painted twice");
    let neighbour = app.tabs.lock().unwrap().pane_by_id(ids[1]).unwrap();
    assert!(
        neighbour
            .lock()
            .unwrap()
            .grid_snapshot()
            .dirty_rows
            .iter()
            .all(|dirty| !dirty),
        "an unchanged cursor dirtied the clean neighbour"
    );
    finish(app, reaper);
}

/// Wheel travel short of a step must not survive letting go of Ctrl, or
/// the next Ctrl+wheel gesture zooms early.
#[test]
fn releasing_ctrl_forgets_partial_wheel_travel() {
    let (mut app, reaper) = test_app();
    let start = app.font().current();
    let travel = |fraction: f32| Wheel::Points(PIXELS_PER_STEP * fraction);
    app.modifiers_changed(Modifiers::CTRL);
    app.wheel(0, travel(0.75));
    assert_eq!(app.font().current(), start, "three quarters is not a step");
    app.modifiers_changed(Modifiers::NONE);
    app.modifiers_changed(Modifiers::CTRL);
    app.wheel(0, travel(0.5));
    assert_eq!(
        app.font().current(),
        start,
        "a new gesture of half a step zoomed: the old three quarters carried over"
    );
    app.wheel(0, travel(0.5));
    assert_ne!(app.font().current(), start, "a whole step zooms");
    finish(app, reaper);
}

fn fill_history(terminal: &Arc<Mutex<Terminal>>) {
    let text = (0..120)
        .map(|n| format!("history-{n}\r\n"))
        .collect::<String>();
    let mut terminal = terminal.lock().unwrap();
    let screen = terminal.grid_snapshot().screen;
    // An isolated grid of the same size: shell startup output can never
    // race the fixture.
    *terminal = Terminal::from_test_vt(screen.cols, screen.rows, text.as_bytes());
}

#[test]
fn wheel_targets_the_hovered_pane_without_focus_or_zoom_travel_leaking() {
    let (mut app, reaper) = test_app();
    let left = app.shape.active_pane;
    app.act(Action::Split(SplitDir::Vertical));
    app.sync();
    let right = app.shape.active_pane;
    assert_ne!(left, right);
    let terminal = app.tabs.lock().unwrap().pane_by_id(left).unwrap();
    fill_history(&terminal);
    app.pointer = Some(pos2(10.0, 40.0));
    let half = Wheel::Points(app.logical_cell().1 / 2.0);
    app.wheel(left, half);
    assert_eq!(app.scroll_pane, Some(left));
    assert_eq!(app.scroll_wheel, 0.5);
    assert_eq!(active_pane(&app.tabs.lock().unwrap()), right);
    app.wheel(left, half);
    assert_eq!(
        terminal.lock().unwrap().display_offset(),
        1,
        "two half-cell deltas scroll the hovered pane"
    );
    let other = app.tabs.lock().unwrap().pane_by_id(right).unwrap();
    assert_eq!(other.lock().unwrap().display_offset(), 0);
    assert_eq!(active_pane(&app.tabs.lock().unwrap()), right);
    app.wheel(left, Wheel::Lines(-1.0));
    assert_eq!(terminal.lock().unwrap().display_offset(), 0);
    app.modifiers_changed(Modifiers::CTRL);
    app.wheel(left, Wheel::Points(PIXELS_PER_STEP / 2.0));
    assert_eq!(app.wheel, 0.5);
    assert_eq!(app.scroll_wheel, 0.0);
    app.modifiers_changed(Modifiers::NONE);
    app.wheel(left, half);
    assert_eq!(app.wheel, 0.0);
    assert_eq!(app.scroll_wheel, 0.5);
    // A different pane starts a new accumulation, even without movement
    // (a tab change beneath the pointer).
    app.wheel(right, half);
    assert_eq!(app.scroll_pane, Some(right));
    assert_eq!(app.scroll_wheel, 0.5);
    finish(app, reaper);
}

/// A viewport change (the wheel, a chord, or `term.scroll` on the core)
/// repaints the pane it moved at the next paint, once, and nothing else.
#[test]
fn viewport_changes_repaint_their_pane_once() {
    let (mut app, reaper) = test_app();
    app.act(Action::Split(SplitDir::Vertical));
    app.sync();
    let id = app.shape.active_pane;
    for pane in app.shape.visible() {
        let terminal = app.tabs.lock().unwrap().pane_by_id(pane).unwrap();
        fill_history(&terminal);
    }
    let terminal = app.tabs.lock().unwrap().pane_by_id(id).unwrap();
    app.paint();
    let neighbour = app.shape.visible().into_iter().find(|p| *p != id).unwrap();
    let neighbour_painted = app.painted[&neighbour];
    app.pointer = Some(pos2(AREA.w - 10.0, 40.0));
    for request in [
        ScrollRequest::PageUp,
        ScrollRequest::Top,
        ScrollRequest::Bottom,
    ] {
        let before = app.painted[&id];
        match request {
            ScrollRequest::PageUp => app.wheel(id, Wheel::Lines(3.0)),
            ScrollRequest::Top => app.act(Action::Scroll(request)),
            _ => terminal.lock().unwrap().scroll_view(request),
        }
        for _ in 0..3 {
            app.sync();
        }
        assert_eq!(app.painted[&id], before, "wakes must not paint");
        let screen = terminal.lock().unwrap().screen(false);
        if request == ScrollRequest::Bottom {
            assert_eq!(terminal.lock().unwrap().display_offset(), 0);
            assert!(screen.cursor_visible);
        } else {
            assert!(terminal.lock().unwrap().display_offset() > 0);
            assert!(!screen.cursor_visible);
        }
        app.paint();
        assert_eq!(app.painted[&id], before + 1);
        assert_eq!(app.painted[&neighbour], neighbour_painted);
        app.paint();
        assert_eq!(app.painted[&id], before + 1, "painted twice");
    }
    finish(app, reaper);
}

/// Each chord drives the tab set as the terminal's keyboard always has,
/// on a real PTY-backed `TabSet`.
#[test]
fn chords_drive_the_tab_set() {
    let mut tabs = layout::test_tabs();
    let first_tab = tabs.active_id();
    let left = active_pane(&tabs);

    assert!(apply(&mut tabs, Action::Split(SplitDir::Vertical)).is_empty());
    assert_eq!(tabs.leaves().len(), 2);
    let right = active_pane(&tabs);
    assert_ne!(right, left, "a split focuses the new pane");

    apply(&mut tabs, Action::CyclePane { forward: true });
    assert_eq!(active_pane(&tabs), left);
    apply(&mut tabs, Action::CyclePane { forward: false });
    assert_eq!(active_pane(&tabs), right);
    assert_eq!(tabs.active_id(), first_tab);

    apply(&mut tabs, Action::Focus(Direction::Left));
    assert_eq!(active_pane(&tabs), left);
    apply(&mut tabs, Action::Focus(Direction::Right));
    assert_eq!(active_pane(&tabs), right);

    apply(&mut tabs, Action::NewTab);
    assert_eq!(tabs.list().len(), 2);
    assert_ne!(tabs.active_id(), first_tab, "a new tab is selected");
    apply(&mut tabs, Action::Cycle { forward: true });
    assert_eq!(
        tabs.active_id(),
        first_tab,
        "cycling wraps to the first tab"
    );

    let removed = apply(&mut tabs, Action::ClosePane);
    assert_eq!(removed.len(), 1, "the closed pane's terminal is torn down");
    assert_eq!(tabs.leaves().len(), 1);
    assert_eq!(active_pane(&tabs), left, "focus falls to the sibling");

    let removed = apply(&mut tabs, Action::CloseTab);
    assert_eq!(removed.len(), 1);
    assert_eq!(tabs.list().len(), 1);

    assert!(
        apply(&mut tabs, Action::FontIncrease).is_empty(),
        "not a tab operation"
    );
    assert_eq!(tabs.list().len(), 1);

    let removed = apply(&mut tabs, Action::Quit);
    assert!(!removed.is_empty());
    assert!(tabs.is_empty());
    assert!(
        apply(&mut tabs, Action::NewTab).is_empty(),
        "nothing opens after quit"
    );
    assert!(tabs.is_empty());
}

/// Each hand-over to the GPU grid feeds the pane's term.snapshot
/// DIAGNOSTIC lines, under their GPU names.
#[test]
fn painting_feeds_the_snapshot_frame_metrics() {
    let (mut app, reaper) = test_app();
    let id = app.shape.active_pane;
    let (terminal, _) = fixture(&mut app, id, 8, 3, b"x");
    app.zoom(|font| font.step_by(1));
    app.paint();
    app.zoom(|font| font.step_by(1));
    app.paint();
    let summary = terminal.lock().unwrap().stats.lock().unwrap().summary();
    assert!(summary.contains("frames=2"), "{summary}");
    assert!(summary.contains("process_frame_interval:n=1"), "{summary}");
    assert!(summary.contains("gpu_prepare_cpu_side:n=1"), "{summary}");
    assert!(
        summary.contains("VT_notification_to_GPU_snapshot"),
        "{summary}"
    );
    drop(terminal);
    finish(app, reaper);
}

/// Quit queues the window's close, and the closed tab set syncs to Exit.
#[test]
fn quit_closes_the_window() {
    let (mut app, reaper) = test_app();
    app.act(Action::Quit);
    assert_eq!(app.effects, vec![Effect::Quit]);
    assert_eq!(app.sync(), Synced::Exit);
    finish(app, reaper);
}
