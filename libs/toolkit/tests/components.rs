// SPDX-License-Identifier: MIT OR Apache-2.0
//! The chrome components' behaviour, driven through kittest: checkboxes
//! and switches, the combo's keyboard, value-field scrubbing, editing and
//! stepping, push-button focus, dialogs, panel groups, document tabs,
//! sliders, canvas scrollbars and the title bar's right-hand group.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, MouseWheelUnit, Pos2, Rect, TouchPhase, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::button::PushButton;
use toolkit::dialog::{Choice, Dialog, Role as DialogRole};
use toolkit::field::ValueField;
use toolkit::slider::Slider;
use toolkit::tabs::{self, DocTab};
use toolkit::toggle::{Checkbox, Switch};
use toolkit::{canvas, combo, panel};

fn harness<S: 'static>(scheme: Scheme, mode: Mode, state: S, app: impl FnMut(&mut egui::Ui, &mut S) + 'static) -> Harness<'static, S> {
    // Steps of a 60th of a second: the harness spaces queued clicks a few
    // steps apart, and its default quarter-second steps would split a
    // double click. Animations then take more steps to settle.
    let builder = Harness::builder().with_size(vec2(640.0, 400.0)).with_step_dt(1.0 / 60.0).with_max_steps(60);
    let mut h = builder.build_ui_state(app, state);
    toolkit::install(&h.ctx, &fixture::theme(scheme, mode));
    h.run();
    h
}

fn pro<S: 'static>(state: S, app: impl FnMut(&mut egui::Ui, &mut S) + 'static) -> Harness<'static, S> {
    harness(Scheme::Pro, Mode::Light, state, app)
}

/// Press at `from`, move to `to` in steps, release there.
fn drag<S>(h: &mut Harness<'_, S>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run();
    }
    h.drop_at(to);
    h.run();
}

#[derive(Default)]
struct Flags {
    checked: bool,
    on: bool,
}

#[test]
fn checkbox_and_switch_toggle_from_their_labels() {
    let mut h = pro(Flags::default(), |ui, s| {
        ui.add(Checkbox::new(&mut s.checked, "Resample"));
        ui.add(Switch::new(&mut s.on, "Pressure for Size"));
    });
    // Click the right end of the row: the label, not the box.
    let row = h.get_by_label("Resample").rect();
    h.hover_at(pos2(row.right() - 4.0, row.center().y));
    h.run();
    h.drag_at(pos2(row.right() - 4.0, row.center().y));
    h.drop_at(pos2(row.right() - 4.0, row.center().y));
    h.run();
    assert!(h.state().checked, "the label is part of the click target");
    h.get_by_label("Pressure for Size").click();
    h.run();
    assert!(h.state().on);
}

#[derive(Default)]
struct Pick {
    index: usize,
    focused: bool,
    changes: u32,
}

const WORKSPACES: [&str; 4] = ["Essentials", "Photography", "Painting", "Graphic and Web"];

#[test]
fn an_open_combo_steps_its_selection_with_the_arrow_keys() {
    let mut h = pro(Pick::default(), |ui, s| {
        let r = combo::show(ui, "workspace", &mut s.index, &WORKSPACES, None);
        s.focused = r.has_focus();
        s.changes += u32::from(r.changed());
    });
    h.get_by_role(Role::ComboBox).click();
    h.run();
    assert!(h.state().focused, "a click focuses the combo");
    let _ = h.get_by_label("Painting");
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().index, 1, "the value changes at once");
    let _ = h.get_by_label("Painting");
    for _ in 0..5 {
        h.key_press(Key::ArrowDown);
        h.run();
    }
    assert_eq!(h.state().index, 3, "clamped at the end");
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(h.state().index, 2);
    assert!(h.state().focused, "the arrows never move focus off the combo");
    assert!(h.state().changes >= 3);
    let _ = h.get_by_label("Essentials");
}

struct Num {
    value: f64,
    rect: Rect,
}

fn value_field(value: f64) -> Harness<'static, Num> {
    pro(Num { value, rect: Rect::NOTHING }, |ui, s| {
        s.rect = ui.add(ValueField::new(&mut s.value, 0.0..=1000.0).unit("%")).rect;
    })
}

#[test]
fn dragging_a_value_field_scrubs_half_a_unit_per_point() {
    let mut h = value_field(100.0);
    let c = h.state().rect.center();
    drag(&mut h, c, c + vec2(20.0, 0.0));
    assert_eq!(h.state().value, 110.0);
    drag(&mut h, c, c - vec2(400.0, 0.0));
    assert_eq!(h.state().value, 0.0, "clamped to the range");
}

#[test]
fn a_value_field_applies_arithmetic_on_enter_and_plain_numbers_as_typed() {
    let mut h = value_field(100.0);
    h.get_by_role(Role::SpinButton).click();
    h.run();
    h.get_by_role(Role::TextInput).type_text("+5");
    h.run();
    assert_eq!(h.state().value, 100.0, "arithmetic waits for Enter");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().value, 105.0);

    h.get_by_role(Role::SpinButton).click();
    h.run();
    h.get_by_role(Role::TextInput).type_text("0");
    h.run();
    assert_eq!(h.state().value, 1000.0, "1050 applies as typed, clamped");
}

#[test]
fn up_and_down_step_after_rounding_to_the_step() {
    let mut h = value_field(55.4);
    h.get_by_role(Role::SpinButton).click();
    h.run();
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(h.state().value, 56.0, "55.4 + 1 = 56");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    h.run();
    assert_eq!(h.state().value, 70.0, "56 rounds to the step of 10 first");
    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowDown);
    h.run();
    assert!((h.state().value - 69.9).abs() < 1e-9, "{}", h.state().value);
}

/// Press and release at `at` in one frame.
fn click_at<S>(h: &mut Harness<'_, S>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.drop_at(at);
    h.run();
}

struct Three {
    values: [f64; 3],
    rects: [Rect; 3],
}

#[test]
fn opening_one_value_field_never_renumbers_the_next() {
    let state = Three { values: [10.0, 20.0, 30.0], rects: [Rect::NOTHING; 3] };
    let mut h = pro(state, |ui, s| {
        for (value, rect) in s.values.iter_mut().zip(&mut s.rects) {
            *rect = ui.add(ValueField::new(value, 0.0..=1000.0)).rect;
        }
    });
    let [first, second, _] = h.state().rects;
    click_at(&mut h, first.center());
    click_at(&mut h, second.center());
    h.get_by_role(Role::TextInput).type_text("7");
    h.run();
    assert_eq!(h.state().values, [10.0, 207.0, 30.0], "the typing reached the second field");
}

struct Mixed {
    before: String,
    value: f64,
    after: String,
    rects: [Rect; 3],
}

#[test]
fn pending_arithmetic_applies_when_focus_moves_before_or_after_the_field() {
    for target in [0, 2] {
        let state = Mixed { before: String::new(), value: 100.0, after: String::new(), rects: [Rect::NOTHING; 3] };
        let mut h = pro(state, |ui, s| {
            s.rects[0] = ui.text_edit_singleline(&mut s.before).rect;
            s.rects[1] = ui.add(ValueField::new(&mut s.value, 0.0..=1000.0)).rect;
            s.rects[2] = ui.text_edit_singleline(&mut s.after).rect;
        });
        let rects = h.state().rects;
        click_at(&mut h, rects[1].center());
        h.event(Event::Text("+5".into()));
        h.run();
        assert_eq!(h.state().value, 100.0, "arithmetic waits");
        click_at(&mut h, rects[target].center());
        h.run();
        assert_eq!(h.state().value, 105.0, "focus to the field {} it", if target == 0 { "before" } else { "after" });
    }
}

#[test]
fn a_non_finite_number_keeps_the_previous_value() {
    let mut h = value_field(100.0);
    h.get_by_role(Role::SpinButton).click();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role(Role::TextInput).type_text("NaN");
    h.run();
    assert_eq!(h.state().value, 100.0, "NaN never lands");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().value, 100.0);
    assert!(h.state().value.is_finite());
}

struct Form {
    name: String,
    width: f64,
    chosen: Vec<usize>,
}

#[test]
fn enter_in_a_dialog_field_never_runs_the_default_button() {
    let state = Form { name: String::new(), width: 100.0, chosen: Vec::new() };
    let mut h = pro(state, |ui, s| {
        let r = Dialog::new("form", "Image Size")
            .buttons(vec![Choice::new("OK", DialogRole::Default), Choice::new("Cancel", DialogRole::Cancel)])
            .show(ui.ctx(), |ui| {
                ui.text_edit_singleline(&mut s.name);
                ui.add(ValueField::new(&mut s.width, 0.0..=1000.0));
            });
        s.chosen.extend(r.chosen);
    });
    h.get_by_role(Role::TextInput).click();
    h.run();
    h.event(Event::Text("Layer".into()));
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().chosen.is_empty(), "Enter left the text field only");
    h.get_by_role(Role::SpinButton).click();
    h.run();
    h.event(Event::Text("+5".into()));
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().chosen.is_empty(), "Enter left the value field only");
    assert_eq!(h.state().width, 105.0, "and committed it");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().chosen, [0], "with nothing focused, Enter runs the default");
}

#[derive(Default)]
struct Pressed {
    clicks: u32,
    focused: bool,
}

#[test]
fn push_buttons_take_focus_from_the_keyboard_only() {
    let mut h = pro(Pressed::default(), |ui, s| {
        let r = ui.add(PushButton::primary("OK"));
        s.clicks += u32::from(r.clicked());
        s.focused = r.has_focus();
    });
    h.get_by_label("OK").click();
    h.run();
    assert_eq!(h.state().clicks, 1);
    assert!(!h.state().focused, "a click never shows the focus ring");
    h.key_press(Key::Tab);
    h.run();
    assert!(h.state().focused, "Tab focuses it");
}

struct Asked {
    chosen: Vec<usize>,
    dismissed: bool,
    rect: Rect,
}

fn dialog() -> Harness<'static, Asked> {
    pro(Asked { chosen: Vec::new(), dismissed: false, rect: Rect::NOTHING }, |ui, s| {
        let r = Dialog::new("size", "Image Size")
            .buttons(vec![Choice::new("OK", DialogRole::Default), Choice::new("Cancel", DialogRole::Cancel)])
            .show(ui.ctx(), |ui| ui.label("Width and height"));
        s.chosen.extend(r.chosen);
        s.dismissed |= r.dismissed;
        s.rect = r.rect;
    })
}

#[test]
fn enter_confirms_and_escape_cancels_a_dialog() {
    let mut h = dialog();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().chosen, [0], "Enter runs the default");
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().chosen, [0, 1], "Esc runs Cancel");
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(h.state().chosen, [0, 1, 1]);
    assert!(!h.state().dismissed);
}

#[test]
fn a_dialog_opens_centred_ignores_clicks_outside_and_moves_by_its_title() {
    let mut h = dialog();
    let first = h.state().rect;
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 400.0));
    // Centred, 1.75 pt low as the evidence measures.
    assert!((first.center() - (screen.center() + vec2(0.0, 1.75))).length() < 4.0, "{first:?}");
    assert!(first.width() >= 380.0 && first.width() <= 440.0 + 2.0 * 7.0, "{first:?}");
    h.get_by_label("Width and height");
    let outside = pos2(4.0, 4.0);
    h.hover_at(outside);
    h.drag_at(outside);
    h.drop_at(outside);
    h.run();
    assert!(h.state().chosen.is_empty(), "a click outside does nothing");
    // The title text, not the Dialog node its title also names.
    assert_eq!(h.get_all_by_role(Role::Dialog).count(), 1, "the dialog is one modal Dialog node");
    let _ = h.get_by_role_and_label(Role::Dialog, "Image Size");
    let title = h.get_by_role_and_label(Role::Label, "Image Size").rect().center();
    drag(&mut h, title, title + vec2(-30.0, 20.0));
    let moved = h.state().rect;
    assert!((moved.min - (first.min + vec2(-30.0, 20.0))).length() < 1.5, "{moved:?} from {first:?}");
}

#[derive(Default)]
struct Dock {
    state: panel::GroupState,
}

#[test]
fn panel_groups_select_and_collapse_from_their_tabs_in_both_styles() {
    for (scheme, mode) in [(Scheme::Pro, Mode::Light), (Scheme::Studio, Mode::Dark)] {
        let mut h = harness(scheme, mode, Dock::default(), |ui, s| {
            let r = panel::group(ui, "dock", &["Layers", "Channels", "Paths"], |ui, tab| {
                ui.label(["Layer body", "Channel body", "Path body"][tab]);
            });
            s.state = r.state;
        });
        let _ = h.get_by_label("Layer body");
        h.get_by_label("Channels").click();
        h.run();
        assert_eq!(h.state().state.selected, 1, "{scheme:?}");
        let _ = h.get_by_label("Channel body");
        // Past the double-click window, then a pair of clicks.
        h.run_steps(30);
        h.get_by_label("Channels").click();
        h.get_by_label("Channels").click();
        h.run();
        assert!(h.state().state.collapsed, "{scheme:?}: a double-click collapses");
        assert!(h.query_by_label("Channel body").is_none());
    }
}

struct Column {
    order: Vec<usize>,
    strips: Vec<Rect>,
}

#[test]
fn dragging_a_group_by_its_strip_reorders_the_dock() {
    for (scheme, mode) in [(Scheme::Pro, Mode::Light), (Scheme::Studio, Mode::Dark)] {
        let state = Column { order: vec![0, 1, 2], strips: vec![Rect::NOTHING; 3] };
        let mut h = harness(scheme, mode, state, |ui, s| {
            let names = ["Color", "Swatches", "Layers"];
            let order = s.order.clone();
            let strips = &mut s.strips;
            let dropped = panel::stack(ui, "dock", &order, |ui, group| {
                let r = panel::group(ui, ("dock", group), &[names[group]], |ui, _| {
                    ui.label(format!("{} body", names[group]));
                    ui.add_space(30.0);
                });
                strips[group] = r.handle.rect;
                r.handle
            });
            if let Some(order) = dropped {
                s.order = order;
            }
        });
        // Grab the third group's handle clear of its tab and menu button and
        // drop it above the first group.
        let strip = h.state().strips[2];
        let grab = pos2(strip.center().x + 40.0, strip.top() + 6.0);
        let top = h.state().strips[0].top();
        drag(&mut h, grab, pos2(grab.x, top + 2.0));
        assert_eq!(h.state().order, [2, 0, 1], "{scheme:?}");
        // A drop back in place changes nothing.
        let strip = h.state().strips[0];
        let grab = pos2(strip.center().x + 40.0, strip.top() + 6.0);
        drag(&mut h, grab, grab + vec2(0.0, 3.0));
        assert_eq!(h.state().order, [2, 0, 1], "{scheme:?}");
    }
}

/// sol round 2, 7: each group keeps its own widgets' ids and scroll state,
/// whatever its position in the stack.
#[test]
fn stacked_groups_keep_their_own_scroll_and_widget_ids_across_a_reorder() {
    use egui_kittest::kittest::NodeT;
    let state = Column { order: vec![0, 1], strips: vec![Rect::NOTHING; 2] };
    let mut h = harness(Scheme::Pro, Mode::Light, state, |ui, s| {
        let names = ["Alpha", "Beta"];
        let order = s.order.clone();
        let strips = &mut s.strips;
        if let Some(order) = panel::stack(ui, "dock", &order, |ui, group| {
            let r = panel::group(ui, ("dock", group), &[names[group]], |ui, _| {
                let _ = ui.button(format!("Apply {}", names[group]));
                // A default (unsalted) scroll area in each group.
                egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                    for row in 0..20 {
                        ui.label(format!("{} row {row}", names[group]));
                    }
                });
            });
            strips[group] = r.handle.rect;
            r.handle
        }) {
            s.order = order;
        }
    });
    let id = |h: &Harness<'_, Column>, label: &str| h.get_by_label(label).accesskit_node().id();
    let (alpha, beta) = (id(&h, "Apply Alpha"), id(&h, "Apply Beta"));
    // Scroll the first group's list only.
    let over = h.get_by_label("Alpha row 1").rect().center();
    h.hover_at(over);
    h.event(Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -200.0), phase: TouchPhase::Move, modifiers: Modifiers::NONE });
    h.run_steps(30);
    let alpha_top = h.get_by_label("Alpha row 0").rect().top();
    let beta_top = h.get_by_label("Beta row 0").rect().top();
    let beta_list = h.get_by_label("Apply Beta").rect().bottom();
    assert!(alpha_top < h.get_by_label("Apply Alpha").rect().bottom(), "Alpha's list scrolled up");
    assert!(beta_top >= beta_list, "Beta's list did not: they share no scroll state");
    // Move Beta above Alpha.
    let strip = h.state().strips[1];
    let grab = pos2(strip.center().x + 40.0, strip.top() + 6.0);
    let top = h.state().strips[0].top();
    drag(&mut h, grab, pos2(grab.x, top + 2.0));
    assert_eq!(h.state().order, [1, 0]);
    assert_eq!((id(&h, "Apply Alpha"), id(&h, "Apply Beta")), (alpha, beta), "widget ids move with their groups");
    assert!(h.get_by_label("Alpha row 0").rect().top() < h.get_by_label("Apply Alpha").rect().bottom(), "Alpha kept its scroll");
}

#[derive(Default)]
struct Docs {
    selected: usize,
    closed: Vec<usize>,
}

#[test]
fn document_tabs_select_and_close() {
    let docs = vec![
        DocTab { title: "Untitled @ 100% (RGB/8)".into(), name: "Untitled".into(), meta: "RGB/8".into(), ..DocTab::default() },
        DocTab { title: "Photo @ 50% (RGB/16)".into(), name: "Photo".into(), meta: "RGB/16".into(), dirty: true, ..DocTab::default() },
    ];
    for (scheme, mode, second) in [(Scheme::Pro, Mode::Dark, "Photo @ 50% (RGB/16)  •"), (Scheme::Studio, Mode::Light, "Photo  •")] {
        let docs = docs.clone();
        let mut h = harness(scheme, mode, Docs::default(), move |ui, s| {
            let events = tabs::documents(ui, egui::Id::new("docs"), &docs, s.selected, None);
            if let Some(index) = events.selected {
                s.selected = index;
            }
            s.closed.extend(events.closed);
        });
        h.get_by_label(second).click();
        h.run();
        assert_eq!(h.state().selected, 1, "{scheme:?}");
        let close = if scheme == Scheme::Pro { "Close Untitled @ 100% (RGB/8)" } else { "Close Untitled" };
        h.get_by_label(close).click();
        h.run();
        assert_eq!(h.state().closed, [0], "{scheme:?}");
        assert_eq!(h.state().selected, 1, "the close box does not select");
    }
}

struct Level {
    value: f64,
    rect: Rect,
}

#[test]
fn sliders_follow_the_pointer_reset_on_double_click_and_step_on_the_wheel() {
    let mut h = pro(Level { value: 20.0, rect: Rect::NOTHING }, |ui, s| {
        ui.set_width(214.0);
        s.rect = ui.add(Slider::new(&mut s.value, 0.0..=200.0).reset_to(100.0).wheel(1.0).label("Opacity")).rect;
    });
    let r = h.state().rect;
    // The track is inset 7 pt each side.
    let at = |t: f32| pos2(egui::lerp(r.left() + 7.0..=r.right() - 7.0, t), r.center().y);
    drag(&mut h, at(0.25), at(0.75));
    assert!((h.state().value - 150.0).abs() < 1.0, "{}", h.state().value);
    for _ in 0..2 {
        h.hover_at(at(0.5));
        h.drag_at(at(0.5));
        h.drop_at(at(0.5));
        h.step();
    }
    h.run();
    assert_eq!(h.state().value, 100.0, "a double-click resets");
    h.hover_at(at(0.5));
    h.run();
    let wheel = |modifiers| Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, 1.0), phase: TouchPhase::Move, modifiers };
    h.event(wheel(Modifiers::NONE));
    h.run();
    assert_eq!(h.state().value, 101.0);
    h.event_modifiers(wheel(Modifiers::SHIFT), Modifiers::SHIFT);
    h.run();
    assert_eq!(h.state().value, 111.0, "x10 with Shift");
}

#[derive(Default)]
struct View {
    offset: Vec2,
}

#[test]
fn clicking_a_canvas_track_pages_and_dragging_the_thumb_scrolls() {
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(412.0, 312.0));
    let mut h = pro(View::default(), move |ui, s| {
        let _ = canvas::scrollbars(ui, egui::Id::new("canvas"), viewport, vec2(1600.0, 1200.0), &mut s.offset);
    });
    // The horizontal track runs along the bottom, 12 pt high.
    let track_y = viewport.bottom() - 6.0;
    let far = pos2(380.0, track_y);
    h.hover_at(far);
    h.drag_at(far);
    h.drop_at(far);
    h.run();
    assert_eq!(h.state().offset.x, 400.0, "one screenful (the 400 pt view) toward the click");
    // The thumb (400 of 1600 on a 394 pt lane) now starts about 98.5 pt in.
    let thumb = pos2(3.0 + 98.5 + 20.0, track_y);
    drag(&mut h, thumb, thumb + vec2(10.0, 0.0));
    let expected = 400.0 + 10.0 / (394.0 - 98.5) * 1200.0;
    assert!((h.state().offset.x - expected).abs() < 1.0, "{} vs {expected}", h.state().offset.x);
}

fn canvas_view() -> Harness<'static, View> {
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(412.0, 312.0));
    pro(View::default(), move |ui, s| {
        let _ = canvas::scrollbars(ui, egui::Id::new("canvas"), viewport, vec2(1600.0, 1200.0), &mut s.offset);
    })
}

#[test]
fn a_thumb_press_drags_however_far_the_first_move_goes_and_a_still_click_on_it_never_pages() {
    let mut h = canvas_view();
    // The thumb: 400 of 1600 on a 394 pt lane, 98.5 long from 3 pt in.
    let thumb = pos2(3.0 + 40.0, 312.0 - 6.0);
    click_at(&mut h, thumb);
    assert_eq!(h.state().offset.x, 0.0, "a click on the thumb does not page");
    // One large move between press and release.
    h.hover_at(thumb);
    h.drag_at(thumb);
    h.run();
    h.hover_at(thumb + vec2(150.0, 0.0));
    h.run();
    h.drop_at(thumb + vec2(150.0, 0.0));
    h.run();
    let expected = 150.0 / (394.0 - 98.5) * 1200.0;
    assert!((h.state().offset.x - expected).abs() < 1.0, "{} vs {expected}", h.state().offset.x);
}

#[test]
fn a_slider_moves_from_the_keyboard_and_from_accesskit() {
    use egui::accesskit::{Action, ActionData, ActionRequest};
    use egui_kittest::kittest::NodeT;
    let mut h = pro(Level { value: 50.0, rect: Rect::NOTHING }, |ui, s| {
        ui.set_width(214.0);
        s.rect = ui.add(Slider::new(&mut s.value, 0.0..=200.0).step(1.0).label("Opacity")).rect;
    });
    h.get_by_label("Opacity").focus();
    h.run();
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(h.state().value, 51.0, "Right steps once");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
    h.run();
    assert_eq!(h.state().value, 41.0, "Shift steps ten");
    let (target_node, target_tree) = h.get_by_label("Opacity").accesskit_node().locate();
    let request = move |action, data| Event::AccessKitActionRequest(ActionRequest { action, target_node, target_tree, data });
    let set = request(Action::SetValue, Some(ActionData::NumericValue(120.0)));
    h.event(set);
    h.run();
    assert_eq!(h.state().value, 120.0, "SetValue");
    let increment = request(Action::Increment, None);
    h.event(increment);
    h.run();
    assert_eq!(h.state().value, 121.0, "Increment");
    let too_far = request(Action::SetValue, Some(ActionData::NumericValue(f64::NAN)));
    h.event(too_far);
    h.run();
    assert_eq!(h.state().value, 121.0, "a non-finite value is refused");
}

/// sol final review, 5: two clicks coalesced into one frame are judged by
/// their own places: far apart they only move the knob; together they are
/// a double click and reset.
#[test]
fn coalesced_clicks_reset_only_when_they_are_near_each_other() {
    let mut h = pro(Level { value: 20.0, rect: Rect::NOTHING }, |ui, s| {
        ui.set_width(214.0);
        s.rect = ui.add(Slider::new(&mut s.value, 0.0..=200.0).reset_to(150.0).label("Opacity")).rect;
    });
    let r = h.state().rect;
    let at = |t: f32| pos2(egui::lerp(r.left() + 7.0..=r.right() - 7.0, t), r.center().y);
    let click = |pos: Pos2| {
        let button = move |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        [Event::PointerMoved(pos), button(true), button(false)]
    };
    h.input_mut().events.extend(click(at(0.1)).into_iter().chain(click(at(0.9))));
    h.run();
    assert!((h.state().value - 180.0).abs() < 1.0, "the second click moved it, with no reset: {}", h.state().value);
    h.run_steps(60);
    h.input_mut().events.extend(click(at(0.3)).into_iter().chain(click(at(0.3))));
    h.run();
    assert_eq!(h.state().value, 150.0, "two near clicks in one frame are a double click");
}

struct Clipped {
    value: f64,
    below: u32,
    slider: Rect,
    button: Rect,
}

/// sol on c817cb8, 1: a slider clipped by a scroll viewport takes no press
/// aimed at a control past the viewport, which gets its click.
#[test]
fn a_clipped_slider_ignores_a_press_on_the_control_past_its_viewport() {
    let state = Clipped { value: 20.0, below: 0, slider: Rect::NOTHING, button: Rect::NOTHING };
    let mut h = pro(state, |ui, s| {
        ui.set_width(214.0);
        // The slider sits wholly below the 30 pt viewport, where the button
        // after the scroll area is drawn.
        egui::ScrollArea::vertical().max_height(30.0).show(ui, |ui| {
            ui.add_space(72.0);
            s.slider = ui.add(Slider::new(&mut s.value, 0.0..=200.0).label("Opacity")).rect;
            ui.add_space(60.0);
        });
        let below = ui.button("Below");
        s.button = below.rect;
        s.below += u32::from(below.clicked());
    });
    let (slider, button) = (h.state().slider, h.state().button);
    let at = button.center();
    assert!(slider.contains(at), "the press lands inside the slider's (clipped) allocation: {slider:?} {button:?}");
    click_at(&mut h, at);
    assert_eq!(h.state().value, 20.0, "the hidden slider did not move");
    assert_eq!(h.state().below, 1, "the button got its click");
}

/// sol: a button drawn over the slider on the same layer is egui's click
/// target there, the slider only a drag candidate. A press held across
/// frames and its release click the button once; the slider stays put.
#[test]
fn a_button_over_a_slider_takes_the_press_and_the_slider_stays() {
    let state = Clipped { value: 20.0, below: 0, slider: Rect::NOTHING, button: Rect::NOTHING };
    let mut h = pro(state, |ui, s| {
        ui.set_width(214.0);
        s.slider = ui.add(Slider::new(&mut s.value, 0.0..=200.0).label("Opacity")).rect;
        let over = Rect::from_center_size(s.slider.center(), vec2(60.0, s.slider.height()));
        let button = ui.put(over, egui::Button::new("Over"));
        s.button = button.rect;
        s.below += u32::from(button.clicked());
    });
    let at = h.state().button.center();
    assert!(h.state().slider.contains(at));
    let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(at), button(true)]);
    h.run();
    assert_eq!(h.state().value, 20.0, "the held press does not move the slider");
    h.input_mut().events.push(button(false));
    h.run();
    assert_eq!(h.state().value, 20.0, "nor does its release");
    assert_eq!(h.state().below, 1, "the button clicked exactly once");
}

#[test]
fn disabling_a_slider_mid_press_drops_the_press() {
    let mut h = pro(Gated { value: 20.0, enabled: true }, |ui, s| {
        ui.set_width(214.0);
        ui.add_enabled(s.enabled, Slider::new(&mut s.value, 0.0..=200.0).label("Opacity"));
    });
    let r = h.get_by_label("Opacity").rect();
    let at = |t: f32| pos2(egui::lerp(r.left() + 7.0..=r.right() - 7.0, t), r.center().y);
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(at(0.5)), button(at(0.5), true)]);
    h.run();
    let pressed = h.state().value;
    assert!((pressed - 100.0).abs() < 1.0, "the press moved it: {pressed}");
    h.state_mut().enabled = false;
    h.input_mut().events.push(button(at(0.5), false));
    h.run();
    h.state_mut().enabled = true;
    h.run();
    h.input_mut().events.push(Event::PointerMoved(at(0.9)));
    h.run();
    assert_eq!(h.state().value, pressed, "no stale press follows the pointer after re-enabling");
}

struct Gated {
    value: f64,
    enabled: bool,
}

#[test]
fn a_disabled_slider_takes_no_accesskit_action_and_offers_none() {
    use egui::accesskit::{Action, ActionData, ActionRequest};
    use egui_kittest::kittest::NodeT;
    let mut h = pro(Gated { value: 50.0, enabled: true }, |ui, s| {
        ui.set_width(214.0);
        ui.add_enabled(s.enabled, Slider::new(&mut s.value, 0.0..=200.0).step(1.0).label("Opacity"));
    });
    let (target_node, target_tree) = h.get_by_label("Opacity").accesskit_node().locate();
    let request = move |action, data| Event::AccessKitActionRequest(ActionRequest { action, target_node, target_tree, data });
    let all = || {
        [
            request(Action::Increment, None),
            request(Action::Decrement, None),
            request(Action::Decrement, None),
            request(Action::SetValue, Some(ActionData::NumericValue(150.0))),
        ]
    };
    let offers = |h: &Harness<'_, Gated>, action| h.get_by_label("Opacity").accesskit_node().data().supports_action(action);
    assert!(offers(&h, Action::SetValue) && offers(&h, Action::Increment), "enabled, it offers them");
    h.event(request(Action::Increment, None));
    h.run();
    assert_eq!(h.state().value, 51.0, "enabled, Increment works");
    // From enabled to disabled.
    h.state_mut().enabled = false;
    h.run();
    for event in all() {
        h.event(event);
    }
    h.run();
    assert_eq!(h.state().value, 51.0, "disabled, nothing moves it");
    for action in [Action::SetValue, Action::Increment, Action::Decrement] {
        assert!(!offers(&h, action), "disabled, it does not offer {action:?}");
    }
}

#[test]
fn a_double_click_resets_but_two_distant_clicks_do_not() {
    let mut h = pro(Level { value: 20.0, rect: Rect::NOTHING }, |ui, s| {
        ui.set_width(214.0);
        s.rect = ui.add(Slider::new(&mut s.value, 0.0..=200.0).reset_to(100.0).label("Opacity")).rect;
    });
    let r = h.state().rect;
    let at = |t: f32| pos2(egui::lerp(r.left() + 7.0..=r.right() - 7.0, t), r.center().y);
    click_at(&mut h, at(0.1));
    click_at(&mut h, at(0.9));
    assert!((h.state().value - 180.0).abs() < 1.0, "two distant clicks only move it: {}", h.state().value);
    h.run_steps(60);
    click_at(&mut h, at(0.8));
    click_at(&mut h, at(0.8));
    assert_eq!(h.state().value, 100.0, "a double-click away from the default resets");
}

#[test]
fn the_right_hand_group_gives_way_link_first_then_icons_and_the_combo_shrinks() {
    use toolkit::titlebar::{COMBO_WIDTH, Slot, fit};
    let slots = [Slot::Combo, Slot::Fixed(28.0), Slot::Fixed(28.0), Slot::Link(70.0)];
    // Everything, the combo at its widest: 130 + 34 + 34 + 76, plus 120 left free.
    assert_eq!(fit(1000.0, &slots), (COMBO_WIDTH.max, 4));
    // The link needs 360 (with the combo at 90 and 120 left free).
    assert_eq!(fit(300.0, &slots), (COMBO_WIDTH.max, 3), "the link goes first");
    // 150: combo and search (96 + 34) fit, the theme toggle (34 more) does not;
    // the combo takes what is left, 150 - 6 - 34.
    assert_eq!(fit(150.0, &slots), (110.0, 2), "then the theme toggle");
    assert_eq!(fit(120.0, &slots), (114.0, 1), "then search");
    assert_eq!(fit(80.0, &slots).1, 0);
}

#[test]
fn title_bar_controls_never_overlap_the_caption_buttons() {
    for width in [760.0, 1100.0, 1440.0] {
        let builder = Harness::builder().with_size(vec2(width, 300.0));
        let h = fixture::harness(builder, &fixture::theme(Scheme::Pro, Mode::Light));
        let minimize = h.get_by_label("Minimize").rect();
        let combo = h.get_by_role(Role::ComboBox).rect();
        assert!(combo.right() <= minimize.left() - 4.0 + 0.5, "{width}: {combo:?} vs {minimize:?}");
        let help = h.get_by_label("Help").rect();
        let mut left = combo.left();
        // Each control that shows sits left of the one before it; at 760
        // only the dropdown fits beside the ten menus.
        for label in ["Search commands (Ctrl+K)", "Toggle Theme", "Community"] {
            let Some(node) = h.query_by_label(label) else { break };
            assert!(node.rect().right() <= left, "{width}: {label}");
            left = node.rect().left();
        }
        assert!(help.right() < left, "{width}: the menus stay clear of the controls");
        if width >= 1440.0 {
            assert!(h.query_by_label("Community").is_some(), "everything fits at 1440");
        }
    }
}
