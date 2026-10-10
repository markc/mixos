// SPDX-License-Identifier: MIT OR Apache-2.0
//! The editor driven through egui: offscreen snapshots of a code view and a
//! wrapped Markdown view, and typing, selection and undo through real input
//! events. Regenerate the images with `UPDATE_SNAPSHOTS=1` and look at them
//! before committing.

use design::{DesignContext, Mode, Scheme};
use editor::local::Document;
use editor::{EditCommand, Motion, Palette, View};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use toolkit::Theme;

const RUST: &str = "\
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::HashMap;

/// Count the words in `text`.
pub fn words(text: &str) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

#[test]
fn counts() {
    assert_eq!(words(\"a b a\")[\"a\"], 2); // 中文 and tabs:\there
}
";

const MARKDOWN: &str = "\
# Field notes

The editor wraps prose at word boundaries, so a long paragraph like this one \
reads as a paragraph instead of a single line running off to the right.

- **Soft wrap** is a view option: the text is unchanged.
- `Code spans` and [links](https://example.org) are highlighted.

A_very_long_identifier_without_any_spaces_breaks_inside_the_word_when_it_must.
";

fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

struct App {
    doc: Document,
    palette: Palette,
    view: View,
}

fn harness(app: App, theme: &Theme, size: egui::Vec2) -> Harness<'static, App> {
    let harness = Harness::builder().with_size(size).wgpu().build_ui_state(
        |ui, app: &mut App| {
            let (palette, view) = (app.palette.clone(), app.view.clone());
            app.doc.show(ui, &palette, &view);
        },
        app,
    );
    toolkit::install(&harness.ctx, theme);
    harness
}

#[test]
fn code_studio_dark() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let mut doc = Document::new(RUST, "rust").unwrap();
    let view = View {
        whitespace: true,
        ..View::default()
    };
    doc.command(
        EditCommand::SelectWord(RUST.find("counts").unwrap() + 1),
        &view,
    )
    .unwrap();
    let app = App {
        doc,
        palette: Palette::from_theme(&theme),
        view,
    };
    let mut h = harness(app, &theme, egui::vec2(640.0, 380.0));
    h.run();
    h.snapshot("code_studio_dark");
}

#[test]
fn markdown_wrapped_pro_light() {
    let theme = theme(Scheme::Pro, Mode::Light);
    let app = App {
        doc: Document::new(MARKDOWN, "markdown").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::prose(),
    };
    let mut h = harness(app, &theme, egui::vec2(420.0, 360.0));
    h.run();
    h.snapshot("markdown_wrapped_pro_light");
}

fn click(h: &mut Harness<'_, App>, at: Pos2) {
    for pressed in [true, false] {
        let input = h.input_mut();
        input.events.push(Event::PointerMoved(at));
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.run();
    }
}

fn key(h: &mut Harness<'_, App>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        });
    }
    h.run();
}

fn text(h: &mut Harness<'_, App>, s: &str) {
    h.input_mut().events.push(Event::Text(s.into()));
    h.run();
}

#[test]
fn typing_selection_and_undo_through_input_events() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("first line\nsecond line\n", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::default(),
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    // Click well right of the first line: the caret goes to its end.
    click(&mut h, pos2(380.0, 15.0));
    assert_eq!(h.state().doc.selection().head, "first line".len());
    text(&mut h, "!");
    key(&mut h, Key::Enter, Modifiers::NONE);
    text(&mut h, "x");
    assert_eq!(h.state().doc.contents(), "first line!\nx\nsecond line\n");
    key(&mut h, Key::ArrowDown, Modifiers::SHIFT);
    key(&mut h, Key::End, Modifiers::SHIFT);
    assert_eq!(h.state().doc.selection().anchor, "first line!\nx".len());
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_ne!(
        h.state().doc.contents(),
        "first line!\nx\nsecond line\n",
        "undo through the shortcut"
    );
    h.state_mut()
        .doc
        .command(
            EditCommand::Move {
                to: Motion::DocStart,
                extend: false,
            },
            &View::default(),
        )
        .unwrap();
    h.run();
    assert_eq!(h.state().doc.selection().head, 0);
}

/// A press and its release delivered in one frame still place the caret.
#[test]
fn a_click_within_one_frame_places_the_caret() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("first line\nsecond line\n", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::default(),
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    let at = pos2(380.0, 15.0);
    let input = h.input_mut();
    input.events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run();
    assert_eq!(h.state().doc.selection().head, "first line".len());
}

/// Two Down presses in one frame move two wrapped rows, and Cut after
/// Select All in one frame cuts the new selection.
#[test]
fn batched_keys_build_on_each_other() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new(&"word ".repeat(60), "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::prose(),
    };
    let mut h = harness(app, &theme, egui::vec2(300.0, 300.0));
    h.run();
    click(&mut h, pos2(150.0, 15.0));
    key(&mut h, Key::Home, Modifiers::COMMAND);
    assert_eq!(h.state().doc.selection().head, 0);
    let row = {
        // One Down press: the head lands on the next row's start column.
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
        let one = h.state().doc.selection().head;
        assert!(one > 0, "moved down a row");
        key(&mut h, Key::ArrowUp, Modifiers::NONE);
        assert_eq!(h.state().doc.selection().head, 0);
        one
    };
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::Key {
            key: Key::ArrowDown,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    h.run();
    assert_eq!(h.state().doc.selection().head, 2 * row, "two rows, not one");
    let input = h.input_mut();
    input.events.push(Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    });
    input.events.push(Event::Cut);
    // Typed after the Cut in the same frame: it must land after it.
    input.events.push(Event::Text("z".into()));
    h.run();
    assert_eq!(
        h.state().doc.contents(),
        "z",
        "Cut took the new selection, then the typing followed, in order"
    );
}

/// Select All, Cut and a click in one frame: the click waits for the Cut.
#[test]
fn a_click_behind_held_keys_waits_for_them() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("abc def\n", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::default(),
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    click(&mut h, pos2(380.0, 15.0));
    let at = pos2(380.0, 15.0);
    let input = h.input_mut();
    input.events.push(Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    });
    input.events.push(Event::Cut);
    input.events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run();
    assert_eq!(h.state().doc.contents(), "", "the Cut ran before the click");
}

fn press_release(h: &mut Harness<'_, App>, at: Pos2) {
    let input = h.input_mut();
    input.events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

fn chord(h: &mut Harness<'_, App>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
}

/// One batch: Select All, Copy, a click at the start of line 2, then typing.
/// The typing lands at the click, after it, not over the selection.
#[test]
fn typing_after_a_held_click_follows_it() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("abc\ndef", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View {
            line_numbers: false,
            ..View::default()
        },
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    click(&mut h, pos2(380.0, 15.0));
    chord(&mut h, Key::A, Modifiers::COMMAND);
    h.input_mut().events.push(Event::Copy);
    press_release(&mut h, pos2(24.0, 34.0));
    h.input_mut().events.push(Event::Text("z".into()));
    h.run();
    assert_eq!(h.state().doc.contents(), "abc\nzdef");
}

/// One batch: select the first line, Cut it, click the start of line 2. The
/// click is measured against the text after the Cut.
#[test]
fn a_held_click_hit_tests_the_edited_text() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("a\nbbbb\ncccc", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View {
            line_numbers: false,
            ..View::default()
        },
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    click(&mut h, pos2(380.0, 15.0));
    key(&mut h, Key::Home, Modifiers::COMMAND);
    chord(&mut h, Key::ArrowDown, Modifiers::SHIFT);
    h.input_mut().events.push(Event::Cut);
    press_release(&mut h, pos2(24.0, 34.0));
    // Each barrier waits a frame for the owner: the Cut, then the click.
    h.run_steps(8);
    assert_eq!(h.state().doc.contents(), "bbbb\ncccc");
    assert_eq!(
        h.state().doc.selection().head,
        5,
        "the start of the new line 2"
    );
}

fn pointer(h: &mut Harness<'_, App>, at: Pos2, pressed: Option<bool>) {
    let input = h.input_mut();
    input.events.push(Event::PointerMoved(at));
    if let Some(pressed) = pressed {
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

/// A double click selects the word; a press that became a drag is no click,
/// so the press after it only places the caret.
#[test]
fn double_clicks_select_words_and_drags_are_not_clicks() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new("alpha bravo charlie", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View {
            line_numbers: false,
            ..View::default()
        },
    };
    let mut h = harness(app, &theme, egui::vec2(400.0, 200.0));
    h.run();
    let on_bravo = pos2(24.0 + 8.0 * 7.5, 15.0);
    for pressed in [true, false, true, false] {
        pointer(&mut h, on_bravo, Some(pressed));
    }
    h.run();
    let sel = h.state().doc.selection();
    assert_eq!(
        (sel.anchor.min(sel.head), sel.anchor.max(sel.head)),
        (6, 11),
        "bravo"
    );
    // Let the double-click window pass, then press, drag away and back.
    h.run_steps(30);
    pointer(&mut h, on_bravo, Some(true));
    h.run();
    pointer(&mut h, pos2(on_bravo.x + 60.0, 15.0), None);
    h.run();
    pointer(&mut h, on_bravo, Some(false));
    pointer(&mut h, on_bravo, Some(true));
    pointer(&mut h, on_bravo, Some(false));
    h.run();
    let sel = h.state().doc.selection();
    assert_eq!(sel.anchor, sel.head, "a caret, not a word");
    // The same, with the press, the wander away and back, and the release
    // all in one frame.
    h.run_steps(30);
    pointer(&mut h, on_bravo, Some(true));
    pointer(&mut h, pos2(on_bravo.x + 60.0, 15.0), None);
    pointer(&mut h, on_bravo, Some(false));
    h.run();
    pointer(&mut h, on_bravo, Some(true));
    pointer(&mut h, on_bravo, Some(false));
    h.run();
    let sel = h.state().doc.selection();
    assert_eq!(sel.anchor, sel.head, "still a caret");
}

/// Right then Down in one frame over wrapped rows: Down starts from where
/// Right left the caret.
#[test]
fn a_vertical_motion_after_another_command_waits_for_it() {
    let theme = theme(Scheme::Studio, Mode::Dark);
    let app = App {
        doc: Document::new(&"word ".repeat(60), "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View::prose(),
    };
    let mut h = harness(app, &theme, egui::vec2(300.0, 300.0));
    h.run();
    click(&mut h, pos2(150.0, 15.0));
    key(&mut h, Key::Home, Modifiers::COMMAND);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let row = h.state().doc.selection().head;
    key(&mut h, Key::Home, Modifiers::COMMAND);
    for k in [Key::ArrowRight, Key::ArrowDown] {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::Key {
                key: k,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
    }
    h.run();
    assert_eq!(
        h.state().doc.selection().head,
        row + 1,
        "column 1 of the next row"
    );
}

#[test]
fn read_only_views_ignore_typing() {
    let theme = theme(Scheme::Studio, Mode::Light);
    let app = App {
        doc: Document::new("fixed", "text").unwrap(),
        palette: Palette::from_theme(&theme),
        view: View {
            read_only: true,
            ..View::default()
        },
    };
    let mut h = harness(app, &theme, egui::vec2(300.0, 120.0));
    h.run();
    click(&mut h, pos2(200.0, 15.0));
    text(&mut h, "no");
    key(&mut h, Key::Backspace, Modifiers::NONE);
    assert_eq!(h.state().doc.contents(), "fixed");
    assert_eq!(h.state().doc.selection().head, 5, "the caret still moves");
}
