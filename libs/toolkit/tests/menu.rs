// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menu-bar behaviour (chrome specification §3.3 and §3.6), driven through
//! the real title bar: open on press, the shared pointer and keyboard
//! highlight, submenus, running commands, hover switching, and the open menu
//! holding back global shortcuts.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fixture::Fixture;
use toolkit::menu::{self, Current, Entry, Menu, Row};

fn window() -> Harness<'static, Fixture> {
    let builder = Harness::builder().with_size(egui::vec2(900.0, 500.0));
    fixture::harness(builder, &fixture::theme(Scheme::Pro, Mode::Light))
}

fn current<S>(h: &Harness<'_, S>) -> Option<Current> {
    menu::current(&h.ctx)
}

/// The open menu's title and highlighted labels, level by level.
fn path(h: &Harness<'_, Fixture>) -> Option<(String, Vec<Option<String>>)> {
    current(h).map(|c| (c.menu, c.path))
}

fn labels(menu: &str, path: &[Option<&str>]) -> Option<(String, Vec<Option<String>>)> {
    Some((menu.to_owned(), path.iter().map(|p| p.map(str::to_owned)).collect()))
}

fn press(h: &mut Harness<'_, Fixture>, key: Key) {
    h.key_press(key);
    h.run();
}

fn open_file(h: &mut Harness<'_, Fixture>) {
    h.get_by_label("File").click();
    h.run();
}

fn pointer(h: &Harness<'_, Fixture>, pos: Pos2, pressed: Option<bool>) {
    h.event(Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
}

#[test]
fn a_press_opens_the_menu_before_the_release() {
    let mut h = window();
    let at = h.get_by_label("File").rect().center();
    pointer(&h, at, Some(true));
    h.run();
    assert_eq!(path(&h), labels("File", &[None]), "open on press");
    pointer(&h, at, Some(false));
    h.run();
    assert!(current(&h).is_some(), "the release that ends the opening press does not close it");
    pointer(&h, at, Some(true));
    pointer(&h, at, Some(false));
    h.run();
    assert!(current(&h).is_none(), "a press on the open menu's title closes it");
}

#[test]
fn down_skips_disabled_rows_and_wraps() {
    let mut h = window();
    open_file(&mut h);
    press(&mut h, Key::ArrowDown);
    assert_eq!(path(&h), labels("File", &[Some("New…")]));
    press(&mut h, Key::ArrowDown);
    assert_eq!(path(&h), labels("File", &[Some("Open…")]), "the disabled row is skipped");
    press(&mut h, Key::ArrowUp);
    press(&mut h, Key::ArrowUp);
    assert_eq!(path(&h), labels("File", &[Some("Save As…")]), "Up wraps to the last enabled row");
}

#[test]
fn right_and_left_move_between_menus_and_submenus() {
    let mut h = window();
    open_file(&mut h);
    for _ in 0..4 {
        press(&mut h, Key::ArrowDown);
    }
    assert_eq!(path(&h), labels("File", &[Some("Open Recent")]));
    press(&mut h, Key::ArrowRight);
    assert_eq!(path(&h), labels("File", &[Some("Open Recent"), Some("Clear Recent Files")]), "into the submenu");
    assert_eq!(current(&h).map(|c| c.depth), Some(1));
    press(&mut h, Key::ArrowLeft);
    assert_eq!(path(&h), labels("File", &[Some("Open Recent")]), "back out, parent still highlighted");
    press(&mut h, Key::ArrowUp);
    press(&mut h, Key::ArrowRight);
    assert_eq!(path(&h), labels("Edit", &[None]), "Right on a command row opens the next menu");
    press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::ArrowLeft);
    assert_eq!(path(&h), labels("Help", &[None]), "Left wraps from File to Help");
    press(&mut h, Key::ArrowRight);
    assert_eq!(path(&h), labels("File", &[None]), "Right wraps from Help to File");
}

#[test]
fn enter_runs_the_highlighted_command_and_escape_closes() {
    let mut h = window();
    open_file(&mut h);
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::Enter);
    assert_eq!(h.state().ran, ["file.open"]);
    assert!(current(&h).is_none(), "running closes the menus");
    open_file(&mut h);
    press(&mut h, Key::Escape);
    assert!(current(&h).is_none());
    assert_eq!(h.state().ran, ["file.open"]);
}

#[test]
fn space_opens_a_submenu_and_runs_inside_it() {
    let mut h = window();
    open_file(&mut h);
    for _ in 0..4 {
        press(&mut h, Key::ArrowDown);
    }
    press(&mut h, Key::Space);
    press(&mut h, Key::Space);
    assert_eq!(h.state().ran, ["file.clear-recent"]);
}

#[test]
fn clicking_a_row_runs_it_and_a_disabled_row_does_nothing() {
    let mut h = window();
    open_file(&mut h);
    h.get_by_label("New from Clipboard").click();
    h.run();
    assert!(h.state().ran.is_empty());
    open_file(&mut h);
    h.get_by_label("Close All").click();
    h.run();
    assert_eq!(h.state().ran, ["file.close-all"]);
    assert!(current(&h).is_none());
}

#[test]
fn press_drag_release_runs_in_one_gesture() {
    let mut h = window();
    let title = h.get_by_label("File").rect().center();
    pointer(&h, title, Some(true));
    h.run();
    let row = h.get_by_label("Save").rect().center();
    pointer(&h, row, None);
    h.run();
    assert_eq!(path(&h), labels("File", &[Some("Save")]), "the pointer highlights as it drags");
    pointer(&h, row, Some(false));
    h.run();
    assert_eq!(h.state().ran, ["file.save"]);
}

#[test]
fn hovering_another_title_switches_only_while_a_menu_is_open() {
    let mut h = window();
    let edit = h.get_by_label("Edit").rect().center();
    h.hover_at(edit);
    h.run();
    assert!(current(&h).is_none(), "hover alone never opens a menu");
    open_file(&mut h);
    h.hover_at(edit);
    h.run();
    assert_eq!(path(&h), labels("Edit", &[None]), "moving onto another title switches");
    // A resting pointer does not undo keyboard switching.
    press(&mut h, Key::ArrowRight);
    assert_eq!(path(&h), labels("Image", &[None]));
}

#[test]
fn a_press_outside_closes_the_menus() {
    let mut h = window();
    open_file(&mut h);
    let outside = egui::pos2(700.0, 400.0);
    pointer(&h, outside, Some(true));
    pointer(&h, outside, Some(false));
    h.run();
    assert!(current(&h).is_none());
}

/// `keys` pressed within one frame.
fn burst(h: &mut Harness<'_, Fixture>, keys: &[Key]) {
    for &key in keys {
        h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    }
    h.run();
}

#[test]
fn keys_in_one_frame_apply_in_order_each_repeat_counting() {
    let mut h = window();
    open_file(&mut h);
    burst(&mut h, &[Key::ArrowDown, Key::ArrowDown]);
    assert_eq!(path(&h), labels("File", &[Some("Open…")]), "two Downs are two steps");
    burst(&mut h, &[Key::Escape, Key::Enter]);
    assert!(current(&h).is_none(), "Escape closes first");
    assert!(h.state().ran.is_empty(), "so the Enter after it runs nothing");
}

#[test]
fn shortcuts_match_their_modifiers_exactly() {
    let mut h = window();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::W);
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().ran, ["file.save-as", "file.close-all"], "never the simpler Ctrl+S, Ctrl+W or Ctrl+Z");
}

#[test]
fn a_menu_taller_than_the_window_scrolls_its_highlight_into_view() {
    let builder = Harness::builder().with_size(egui::vec2(900.0, 200.0));
    let mut h = fixture::harness(builder, &fixture::theme(Scheme::Pro, Mode::Light));
    open_file(&mut h);
    assert!(h.get_by_label("Save As…").rect().bottom() > 200.0, "the fixture's File menu overflows");
    press(&mut h, Key::ArrowUp);
    assert_eq!(path(&h), labels("File", &[Some("Save As…")]));
    assert!(h.get_by_label("Save As…").rect().bottom() <= 200.0, "the last row scrolled into view");
    h.get_by_label("Save As…").click();
    h.run();
    assert_eq!(h.state().ran, ["file.save-as"], "and the pointer reaches it");
}

#[test]
fn a_highlight_on_a_row_that_disappears_is_dropped() {
    let rows = |n: usize| (0..n).map(|i| Entry::Row(Row::command(["a", "b", "c"][i], ["A", "B", "C"][i], None, true))).collect();
    let model = vec![Menu { title: "File".into(), entries: rows(3) }];
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 300.0)).build_ui_state(
        |ui, (menus, ran): &mut (Vec<Menu>, Vec<&'static str>)| {
            ui.horizontal(|ui| ran.extend(menu::bar(ui, menus)));
        },
        (model, Vec::new()),
    );
    toolkit::install(&h.ctx, &fixture::theme(Scheme::Pro, Mode::Light));
    h.run();
    h.get_by_label("File").click();
    h.run();
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(current(&h).map(|c| c.path), Some(vec![Some("C".into())]));
    h.state_mut().0[0].entries = rows(2);
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(current(&h).map(|c| c.path), Some(vec![None]), "the stale highlight is gone, nothing ran");
    assert!(h.state().1.is_empty());
}

fn short_window(scheme: Scheme) -> Harness<'static, Fixture> {
    let builder = Harness::builder().with_size(egui::vec2(900.0, 170.0));
    fixture::harness(builder, &fixture::theme(scheme, Mode::Light))
}

#[test]
fn dragging_a_scroll_bar_keeps_the_menu_open() {
    // Classic has solid bars beside the rows; Pro thin floating ones whose
    // strip lies partly beyond the rows' edge.
    for scheme in [Scheme::Classic, Scheme::Pro] {
        let mut h = short_window(scheme);
        open_file(&mut h);
        let row = h.get_by_label("New…").rect();
        let s = h.ctx.global_style().spacing.scroll;
        let allocated = if s.floating { s.floating_allocated_width } else { s.bar_inner_margin + s.bar_width + s.bar_outer_margin };
        assert!(allocated > 0.0, "{scheme:?}: the bar has a strip beyond the rows");
        let at = egui::pos2(row.right() + allocated - s.bar_outer_margin - 1.0, row.center().y);
        pointer(&h, at, Some(true));
        h.run();
        pointer(&h, at + egui::vec2(0.0, 30.0), None);
        h.run_steps(5);
        pointer(&h, at + egui::vec2(0.0, 30.0), Some(false));
        h.run_steps(5);
        assert!(current(&h).is_some(), "{scheme:?}: a scroll bar release neither closes the menu");
        assert!(h.state().ran.is_empty(), "{scheme:?}: nor runs a row");
    }
}

#[test]
fn the_keyboard_enters_a_submenu_scrolled_out_of_view() {
    let ids = ["r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9"];
    let mut entries: Vec<Entry> = ids.iter().map(|&id| Entry::Row(Row::command(id, id, None, true))).collect();
    entries.push(Entry::Row(Row::submenu("More", vec![Entry::Row(Row::command("x", "X", None, true)), Entry::Row(Row::command("y", "Y", None, true))])));
    let model = vec![Menu { title: "File".into(), entries }];
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 170.0)).build_ui_state(
        |ui, (menus, ran): &mut (Vec<Menu>, Vec<&'static str>)| {
            ui.horizontal(|ui| ran.extend(menu::bar(ui, menus)));
        },
        (model, Vec::new()),
    );
    toolkit::install(&h.ctx, &fixture::theme(Scheme::Pro, Mode::Light));
    h.run();
    h.get_by_label("File").click();
    h.run();
    for key in [Key::ArrowUp, Key::ArrowRight] {
        h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(5);
    let c = current(&h).expect("open");
    assert_eq!((c.path, c.depth), (vec![Some("More".into()), Some("X".into())], 1), "the submenu stays open with the keyboard in it");
    h.key_press(Key::ArrowDown);
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().1, ["y"], "and takes the next keys");
}

#[test]
fn a_submenu_closes_when_its_row_scrolls_out_of_view() {
    let mut h = short_window(Scheme::Pro);
    open_file(&mut h);
    h.hover_at(h.get_by_label("Open Recent").rect().center());
    h.run();
    assert_eq!(current(&h).map(|c| c.path.len()), Some(2), "hovering shows the submenu");
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -2000.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    // Wheel scrolling animates over several frames.
    h.run_steps(30);
    let bar_bottom = h.get_by_label("File").rect().bottom();
    assert!(h.get_by_label("Open Recent").rect().center().y < bar_bottom, "the row scrolled up out of view");
    assert_eq!(current(&h).map(|c| c.path.len()), Some(1), "and took its submenu with it");
}

fn open_help(h: &mut Harness<'_, Fixture>) {
    h.get_by_label("Help").click();
    h.run();
}

fn typed(h: &mut Harness<'_, Fixture>, text: &str) {
    h.event(Event::Text(text.to_owned()));
    h.run();
}

#[test]
fn help_opens_with_a_focused_empty_search_that_lists_ranked_matches() {
    let mut h = window();
    open_help(&mut h);
    let field = h.get_by_role(Role::TextInput);
    assert!(field.is_focused(), "the field has the keyboard as Help opens");
    typed(&mut h, "clo");
    // "Close", "Close All" and "Close Others" start with it.
    let _ = h.get_by_label("File › Close");
    let _ = h.get_by_label("File › Close Others");
    let _ = h.get_by_label("Help").rect();
    press(&mut h, Key::Enter);
    assert_eq!(h.state().ran, ["file.close"], "Enter runs the first enabled result");
    assert!(current(&h).is_none(), "and closes the menus");
    open_help(&mut h);
    assert_eq!(h.get_by_role(Role::TextInput).value().as_deref(), Some(""), "cleared each time it opens");
}

#[test]
fn space_and_left_right_edit_the_query_while_up_down_move_the_highlight() {
    let mut h = window();
    open_help(&mut h);
    typed(&mut h, "close");
    // A Space key press arrives with its text: the key must not activate.
    h.key_press(Key::Space);
    typed(&mut h, " a");
    assert!(h.state().ran.is_empty(), "Space never runs a result while typing");
    assert_eq!(h.get_by_role(Role::TextInput).value().as_deref(), Some("close a"), "Space types a space");
    press(&mut h, Key::ArrowLeft);
    assert_eq!(path(&h).map(|p| p.0), Some("Help".to_owned()), "Left stays in the field, Help stays open");
    press(&mut h, Key::ArrowDown);
    assert_eq!(path(&h), labels("Help", &[Some("File › Close All")]), "Down highlights the first result");
    press(&mut h, Key::Enter);
    assert_eq!(h.state().ran, ["file.close-all"]);
}

#[test]
fn nothing_matching_says_so_and_a_click_inside_help_keeps_it_open() {
    let mut h = window();
    open_help(&mut h);
    typed(&mut h, "zzz");
    let empty = h.get_by_role(Role::TextInput).rect();
    let _ = h.get_by_label("No matching commands");
    pointer(&h, empty.center(), Some(true));
    h.run();
    pointer(&h, empty.center(), Some(false));
    h.run();
    assert_eq!(path(&h).map(|p| p.0), Some("Help".to_owned()), "a click in the field keeps Help open");
    press(&mut h, Key::Enter);
    assert!(h.state().ran.is_empty(), "Enter with no results runs nothing");
}

/// Text and then Enter in one frame (one RawInput).
fn type_then_enter(h: &mut Harness<'_, Fixture>, text: &str) {
    let enter = |pressed| Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([Event::Text(text.to_owned()), enter(true), enter(false)]);
    h.run();
}

#[test]
fn enter_in_the_same_frame_as_typing_acts_on_the_new_query() {
    let mut h = window();
    open_help(&mut h);
    typed(&mut h, "clo");
    type_then_enter(&mut h, "zzz");
    assert!(h.state().ran.is_empty(), "\"clozzz\" matches nothing, so Enter runs nothing");
    assert_eq!(h.get_by_role(Role::TextInput).value().as_deref(), Some("clozzz"));
    let _ = h.get_by_label("No matching commands");

    let mut h = window();
    open_help(&mut h);
    typed(&mut h, "clo");
    type_then_enter(&mut h, "se a");
    assert_eq!(h.state().ran, ["file.close-all"], "\"close a\" is Close All, not the stale first result Close");
}

/// sol final review, 4: typing, Enter and a press outside Help in one
/// RawInput. The press closes Help this frame; the held-back Enter belongs
/// to Help's field and is dropped, never replayed into the application.
#[test]
fn enter_held_back_for_help_is_dropped_when_a_press_closes_it() {
    let mut h = window();
    open_help(&mut h);
    typed(&mut h, "clo");
    let outside = Pos2::new(450.0, 400.0);
    let enter = |pressed| Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let button = |pressed| Event::PointerButton { pos: outside, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([
        Event::Text("se".into()),
        enter(true),
        enter(false),
        Event::PointerMoved(outside),
        button(true),
        button(false),
    ]);
    h.run();
    assert!(current(&h).is_none(), "the press outside closed Help");
    assert!(h.state().ran.is_empty(), "nothing ran: {:?}", h.state().ran);
    // Nothing is left to replay later either.
    h.run_steps(3);
    assert!(h.state().ran.is_empty(), "nothing ran later: {:?}", h.state().ran);
}

#[test]
fn search_ranks_label_prefix_word_prefix_contains_then_path() {
    let row = |id: &'static str, label: &str| Entry::Row(Row::command(id, label, None, true));
    let menus = vec![
        Menu { title: "Image".into(), entries: vec![row("a", "Rotate Canvas"), row("b", "Canvas Size…"), row("c", "Precanvas")] },
        Menu { title: "Canvas".into(), entries: vec![row("d", "Flip")] },
    ];
    let ids: Vec<_> = menu::matching(&menus, "canvas").iter().map(|r| r.id.unwrap()).collect();
    assert_eq!(ids, ["b", "a", "c", "d"]);
    assert_eq!(menu::matching(&menus, "flip")[0].label, "Canvas › Flip");
    assert!(menu::matching(&menus, "  ").is_empty());
}

#[test]
fn an_open_menu_holds_back_global_shortcuts() {
    let mut h = window();
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run();
    assert_eq!(h.state().ran, ["file.new"], "the shortcut works with no menu open");
    open_file(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run();
    assert_eq!(h.state().ran, ["file.new"], "but not while a menu has the keyboard");
}
