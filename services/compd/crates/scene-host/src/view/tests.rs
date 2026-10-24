// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The scene view through a real (headless) egui runtime: one pass lays a
//! scene out at a surface size, and the tests read back the rectangles the
//! scene was drawn and hit-tested at.

use std::sync::{Arc, Mutex};

use serde_json::json;
use ui::UiRuntime;
use ui::input::{Input, key};

use super::*;
use crate::templates::PreparedLists;

fn content(source: &str, frame: Option<Frame>, dialog: bool) -> Arc<Content> {
    let tree = scene::resolve(&scene::parse(source).unwrap()).unwrap();
    let lists = crate::templates::validate_templates(&tree).unwrap();
    Arc::new(Content {
        tree,
        lists,
        revision: 1,
        frame,
        dialog,
    })
}

fn runtime(source: &str, size: (u32, u32)) -> UiRuntime<SceneUi> {
    let mut rt = UiRuntime::headless(SceneUi::new(content(source, None, false)), size, 1.0);
    rt.tick();
    rt
}

fn rect_of(rt: &UiRuntime<SceneUi>, id: &str) -> egui::Rect {
    *rt.program
        .layout()
        .nodes
        .get(id)
        .unwrap_or_else(|| panic!("node {id} not drawn"))
}

fn row_of(rt: &UiRuntime<SceneUi>, list: &str, item: &str) -> egui::Rect {
    rt.program.layout().instances[list][item]
}

fn long_toast() -> String {
    include_str!("../../../../../../share/scenes/toast/scene.mix")
        // Exercise the original single title too: the behaviour guard must
        // not mask the renderer regression.
        .replace("t_sum: {\"children\":[\"s_l0\",\"s_l1\",\"s_l2\"],\"gap\":2,\"widget\":\"column\"}", "t_sum: {\"bold\":true,\"elide\":true,\"fill\":true,\"size\":14,\"text\":\"= $model.summary\",\"widget\":\"text\"}")
        .replace("\"summary\":\"\"", &format!("\"summary\":\"{}\"", "W".repeat(120)))
        .replace("\"app\":\"\"", &format!("\"app\":\"{}\"", "W".repeat(120)))
        .replace("\"shown\":[false,false,false,false]", "\"shown\":[true,false,false,false]")
        .replace("\"labels\":[\"\",\"\",\"\",\"\"]", "\"labels\":[\"Got it\",\"\",\"\",\"\"]")
}

fn guarded_toast() -> String {
    include_str!("../../../../../../share/scenes/toast/scene.mix")
        .replace("\"summary_lines\":[\"\",\"\",\"\"]", &format!("\"summary_lines\":[\"{}\",\"{}\",\"{}…\"]", "W".repeat(38), "W".repeat(38), "W".repeat(37)))
        .replace("\"summary_shown\":[false,false,false]", "\"summary_shown\":[true,true,true]")
        .replace("\"app\":\"\"", &format!("\"app\":\"{}\"", "W".repeat(120)))
        .replace("\"shown\":[false,false,false,false]", "\"shown\":[true,false,false,false]")
        .replace("\"labels\":[\"\",\"\",\"\",\"\"]", "\"labels\":[\"Got it\",\"\",\"\",\"\"]")
}

#[test]
fn guarded_toast_fits_the_behaviour_seat_height() {
    let rt = runtime(&guarded_toast(), (360, 154));
    let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 154.0));
    for placed in &rt.program.layout().placed {
        assert!(bounds.contains_rect(placed.rect), "{}: {:?}", placed.id, placed.rect);
    }
}

#[test]
fn wrapped_text_bounds_lines_and_grows_nested_parents() {
    for axis in ["row", "column"] {
        for fill in [false, true] {
            let source = format!(
                "---\nscene: 1\nname: wrap\ncitizen: test\n---\n```mix\nroot: {{widget: \"row\", children: [\"outer\"]}}\nouter: {{widget: \"column\", children: [\"inner\", \"button\"]}}\ninner: {{widget: \"{axis}\", children: [\"title\"]}}\ntitle: {{widget: \"text\", text: \"{}\", size: 14, wrap: true, max_lines: 3, fill: {fill}}}\nbutton: {{widget: \"button\", label: \"Got it\"}}\n```",
                "Wide title ".repeat(30)
            );
            let rt = runtime(&source, (180, 300));
            let title = rect_of(&rt, "title");
            assert!(title.width() <= 180.0, "{axis}/{fill}: {title:?}");
            assert!(
                (title.height() - 50.4).abs() < 1.0,
                "{axis}/{fill}: {title:?}"
            );
            assert!(rect_of(&rt, "inner").height() >= title.height());
            assert!(rect_of(&rt, "button").top() >= title.bottom());
            assert!(rect_of(&rt, "outer").width() <= 180.0);
        }
    }
    let measure = |s: &str| s.chars().count() as f32 * 10.0;
    let lines = flex::wrap_text("one two three four five six", 70.0, 2, &measure);
    assert_eq!(lines.len(), 2);
    assert!(lines[1].ends_with('…'));
    assert!(lines.iter().all(|line| measure(line) <= 70.0));
    assert_eq!(flex::wrap_text("short", 70.0, 3, &measure), ["short"]);
    assert!(
        flex::wrap_text(&"W".repeat(120), 70.0, 3, &measure)
            .iter()
            .all(|line| measure(line) <= 70.0)
    );
}

#[test]
fn long_toast_keeps_controls_visible_and_clickable() {
    for fill in [true, false] {
        let source = if fill {
            long_toast()
        } else {
            long_toast().replace("\"fill\":true,\"size\":14", "\"size\":14")
        };
        let mut rt = runtime(&source, (360, 132));
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 132.0));
        for id in [
            "card",
            "head",
            "t_sum",
            "t_app",
            "t_actions",
            "t_close",
            "b0",
        ] {
            let rect = rect_of(&rt, id);
            assert!(bounds.contains_rect(rect), "{id}: {rect:?}");
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        rt.set_message_handler(move |message: &SceneMessage| {
            if let Some((_, handler, _)) = event_of(message) {
                sink.lock().unwrap().push(handler.to_owned());
            }
        });
        for id in ["t_close", "b0"] {
            let at = rect_of(&rt, id).center();
            rt.queue_input(Input::PointerMoved { x: at.x, y: at.y });
            rt.tick();
            for pressed in [true, false] {
                rt.queue_input(Input::Button {
                    code: 0x110,
                    pressed,
                });
                rt.tick();
            }
        }
        assert_eq!(*seen.lock().unwrap(), ["toast.dismiss", "toast.press0"]);
    }
}

#[test]
fn long_toast_offscreen() {
    use ui::UiProgram;
    let mut scene = SceneUi::new(content(&long_toast(), None, false));
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(360.0, 132.0))
        .wgpu()
        .build_ui(move |ui| {
            if !setup {
                scene.setup(ui.ctx());
                setup = true;
                ui.ctx().request_repaint();
                return;
            }
            scene.show(ui, &mut Vec::new());
            for (id, size, bold) in [("t_sum", 14.0, true), ("t_app", 11.0, false)] {
                let text = paint::elide_end(
                    ui,
                    &"W".repeat(120),
                    &paint::font(size, false, bold),
                    scene.layout.nodes[id].width(),
                );
                assert!(text.ends_with('…'), "{id}: {text}");
            }
        });
    harness.run();
    let image = harness.render().unwrap();
    if let Some(path) = std::env::var_os("TOAST_SNAPSHOT") {
        image.save(path).unwrap();
    }
}

#[test]
fn guarded_toast_offscreen() {
    use ui::UiProgram;
    let mut scene = SceneUi::new(content(&guarded_toast(), None, false));
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(360.0, 154.0))
        .wgpu()
        .build_ui(move |ui| {
            if !setup {
                scene.setup(ui.ctx());
                setup = true;
                ui.ctx().request_repaint();
                return;
            }
            scene.show(ui, &mut Vec::new());
        });
    harness.run();
    let image = harness.render().unwrap();
    if let Some(path) = std::env::var_os("TOAST_GUARD_SNAPSHOT") {
        image.save(path).unwrap();
    }
}

/// The placed text node `id` inside list `list`'s row `item`.
fn text_in_row(rt: &UiRuntime<SceneUi>, list: &str, item: &str, id: &str) -> egui::Rect {
    rt.program
        .layout()
        .placed
        .iter()
        .find(|placed| {
            placed.id == id
                && placed.source
                    == flex::Source::Row {
                        list: list.into(),
                        item: item.into(),
                    }
        })
        .map(|placed| placed.rect)
        .unwrap_or_else(|| panic!("{list}/{item}/{id} not drawn"))
}

#[test]
fn appearance_keeps_content_local_edits_and_toggles_and_updates_the_page() {
    let mut ui = SceneUi::new(content(
        "---\nscene: 1\nname: settings\ncitizen: test\n---\n```mix\nroot: {widget: \"column\", children: [\"entry\", \"enabled\", \"save\"]}\nentry: {widget: \"field\", value: \"authored\"}\nenabled: {widget: \"toggle\", label: \"Enabled\", value: false}\nsave: {widget: \"button\", label: \"Save\", tone: \"primary\"}\n```\n",
        None,
        true,
    ));
    let content = Arc::clone(&ui.content);
    let route = Arc::new(Route::new(
        &content.tree,
        "entry",
        &content.tree.nodes["entry"],
    ));
    ui::UiProgram::update(
        &mut ui,
        SceneMessage::Input(
            Arc::clone(&route),
            "/root/entry".into(),
            "unsaved edit".into(),
        ),
    );
    ui::UiProgram::update(
        &mut ui,
        SceneMessage::Toggle(route, "/root/enabled".into(), true),
    );
    ui.apply_appearance(crate::appearance::fixture("light", 1.0));
    let light = ui.page();
    ui::UiProgram::update(
        &mut ui,
        SceneMessage::Appearance(crate::appearance::fixture("dark", 1.5)),
    );
    assert!(Arc::ptr_eq(&content, &ui.content));
    assert_eq!(ui.edits["/root/entry"], "unsaved edit");
    assert!(ui.toggles["/root/enabled"]);
    assert_ne!(ui.page(), light);
    assert!(
        ui.text_styles().ui.size > TEXT_SIZE,
        "text scale 1.5 applies"
    );
}

#[test]
fn panel_has_square_pager_two_clock_lines_and_full_task_hit_targets() {
    let rt = runtime(
        include_str!("../../tests/fixtures/panel-render.scene.mix"),
        (1536, 52),
    );
    for id in ["1", "2", "3", "4"] {
        let cell = row_of(&rt, "pager", id);
        assert!(
            (cell.width() - cell.height()).abs() < 0.1,
            "pager {id}: {cell:?}"
        );
        assert!((cell.height() - 30.0).abs() < 0.1, "pager {id}: {cell:?}");
    }
    let time = rect_of(&rt, "clock_time");
    let date = rect_of(&rt, "clock_date");
    assert!(
        time.height() >= 22.0 && date.height() >= 13.0,
        "clock: {time:?}, {date:?}"
    );
    assert!(
        date.top() >= time.bottom() - 0.1 && date.bottom() <= 48.0,
        "clock: {time:?}, {date:?}"
    );
    let task = row_of(&rt, "tasks", "task");
    assert!(
        task.width() >= 218.0 && task.height() >= 40.0,
        "task: {task:?}"
    );
}

/// A panel too narrow for its tasks shrinks them (their titles elide) and
/// keeps everything on the edge, instead of pushing the right end off it.
#[test]
fn a_crowded_panel_shrinks_its_tasks_and_stays_on_screen() {
    let width = 450.0;
    let rt = runtime(
        include_str!("../../tests/fixtures/panel-render.scene.mix"),
        (width as u32, 52),
    );
    let task = row_of(&rt, "tasks", "task");
    assert!(task.width() < 218.0, "the task gave way: {task:?}");
    // Every drawn rectangle, the task cells and their contents included.
    for placed in &rt.program.layout().placed {
        assert!(
            placed.rect.right() <= width + 0.5,
            "{} runs off the edge: {:?}",
            placed.key,
            placed.rect
        );
    }
    assert_eq!(
        rect_of(&rt, "clock").right(),
        width - 4.0,
        "clock ends at the padding"
    );
}

/// Crowding never shrinks a cell below its instance's floor: square
/// fixed-height cells and equal width bounds keep their width, and cells
/// never overlap.
#[test]
fn crowded_horizontal_cells_keep_their_floors_and_never_overlap() {
    let source = |template: &str| {
        format!(
            "---\nscene: 1\nname: crowd\ncitizen: c\n---\n```mix\nroot: {{widget: \"row\", children: [\"list\"]}}\nlist: {{widget: \"list\", flow: \"horizontal\", row: \"cell\", row_height: 30, gap: 4, rows: [{{id: \"a\", cells: [\"1\"]}}, {{id: \"b\", cells: [\"2\"]}}, {{id: \"c\", cells: [\"3\"]}}, {{id: \"d\", cells: [\"4\"]}}]}}\ncell: {template}\nlabel: {{widget: \"text\", text: \"{{0}}\"}}\n```\n"
        )
    };
    for (template, floor) in [
        (
            "{widget: \"row\", height: 30, justify: \"center\", padding: 8, children: [\"label\"]}",
            30.0,
        ),
        (
            "{widget: \"row\", min_width: 60, max_width: 60, children: [\"label\"]}",
            60.0,
        ),
    ] {
        let rt = runtime(&source(template), (80, 40));
        let mut cells: Vec<egui::Rect> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| row_of(&rt, "list", id))
            .collect();
        cells.sort_by(|x, y| x.left().total_cmp(&y.left()));
        for cell in &cells {
            assert!(cell.width() >= floor - 0.5, "{template}: {cell:?}");
        }
        for pair in cells.windows(2) {
            assert!(
                pair[1].left() >= pair[0].right() - 0.5,
                "{template}: overlap {pair:?}"
            );
        }
    }
    // A square floor above the row's maximum is held to the maximum,
    // crowded or not.
    let bounded =
        "{widget: \"row\", height: 30, justify: \"center\", max_width: 20, children: [\"label\"]}";
    for surface in [80, 400] {
        let rt = runtime(&source(bounded), (surface, 40));
        for id in ["a", "b", "c", "d"] {
            let cell = row_of(&rt, "list", id);
            assert!(cell.width() <= 20.5, "{surface}: {id} {cell:?}");
        }
    }
}

/// `shrink`, `basis` and `align_self` act as their flexbox names say.
#[test]
fn shrink_basis_and_align_self_follow_flexbox() {
    let source = "---\nscene: 1\nname: items\ncitizen: c\n---\n```mix\nroot: {widget: \"row\", height: 40, children: [\"keep\", \"give\", \"low\"]}\nkeep: {widget: \"column\", basis: 80, shrink: 0, min_width: 0, children: []}\ngive: {widget: \"column\", basis: 80, min_width: 0, children: []}\nlow: {widget: \"column\", min_width: 10, max_width: 10, min_height: 10, max_height: 10, align_self: \"end\", children: []}\n```\n";
    let rt = runtime(source, (120, 40));
    assert_eq!(
        rect_of(&rt, "keep").width(),
        80.0,
        "shrink 0 keeps its basis"
    );
    assert!(rect_of(&rt, "give").width() < 80.0, "the other gives way");
    assert_eq!(rect_of(&rt, "low").bottom(), 40.0, "align_self end");
}

#[test]
fn panel_pager_digits_sit_at_the_centre_of_their_hit_targets() {
    let rt = runtime(
        include_str!("../../tests/fixtures/panel-render.scene.mix"),
        (1536, 52),
    );
    let template = rt.program.content.tree.nodes["pager"]
        .ports
        .get("row")
        .and_then(serde_json::Value::as_str)
        .unwrap()
        .to_owned();
    for id in ["1", "2", "3", "4"] {
        let cell = row_of(&rt, "pager", id);
        // The digit: the row instance's text node.
        let instance = &rt.program.content.lists["pager"].instances[id];
        let text_id = instance
            .iter()
            .find(|(_, node)| node.family == "text")
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("pager row {id} ({template}) has no text"));
        let digit = text_in_row(&rt, "pager", id, &text_id);
        assert!(
            (digit.center().x - cell.center().x).abs() < 0.6,
            "pager {id}: digit={digit:?}, cell={cell:?}"
        );
        assert!(
            (digit.center().y - cell.center().y).abs() < 0.6,
            "pager {id}: digit={digit:?}, cell={cell:?}"
        );
        assert!(digit.left() >= cell.left() - 0.1 && digit.right() <= cell.right() + 0.1);
    }
}

#[test]
fn bounded_centred_groups_keep_their_gaps_and_centre_their_content() {
    let source = "---\nscene: 1\nname: centred\ncitizen: test\nwindow: {\"kind\":\"edge\",\"edge\":\"bottom\",\"h\":220}\n---\n```mix\nroot: {widget: \"column\", fill: true, children: [\"r\", \"c\"]}\nr: {widget: \"row\", height: 30, min_width: 60, max_width: 80, padding: 4, gap: 6, justify: \"center\", align: \"center\", children: [\"a\", \"b\"]}\nc: {widget: \"column\", min_width: 60, max_width: 60, min_height: 60, max_height: 80, padding: 4, gap: 6, justify: \"center\", align: \"center\", children: [\"cc\", \"dd\"]}\na: {widget: \"text\", text: \"1\", size: 10}\nb: {widget: \"text\", text: \"2\", size: 10}\ncc: {widget: \"text\", text: \"1\", size: 10}\ndd: {widget: \"text\", text: \"2\", size: 10}\n```\n";
    for width in [120, 200] {
        let rt = runtime(source, (width, 220));
        let row = rect_of(&rt, "r");
        let column = rect_of(&rt, "c");
        let [a, b, c, d] = ["a", "b", "cc", "dd"].map(|id| rect_of(&rt, id));
        assert!(
            ((a.left() + b.right()) / 2.0 - row.center().x).abs() < 0.1,
            "row={row:?}, a={a:?}, b={b:?}"
        );
        assert!(
            ((c.top() + d.bottom()) / 2.0 - column.center().y).abs() < 0.1,
            "column={column:?}, c={c:?}, d={d:?}"
        );
        assert!((b.left() - a.right() - 6.0).abs() < 0.1);
        assert!((d.top() - c.bottom() - 6.0).abs() < 0.1);
        // min_width holds against the short content; min = max fixes the column.
        // Flexbox (taffy, as Quoin): a fit-content row inside its bounds.
        assert!(
            row.width() >= 60.0 - 0.1 && row.width() <= 80.0 + 0.1,
            "row={row:?}"
        );
        assert!(
            (column.width() - 60.0).abs() < 0.1 && (column.height() - 60.0).abs() < 0.1,
            "column={column:?}"
        );
        assert!(a.left() >= row.left() && b.right() <= row.right());
    }
}

#[test]
fn autofocus_targets_the_field_and_typing_emits_filter() {
    let source = "---\nscene: 1\nname: launcher\ncitizen: test\nwindow: {\"kind\":\"edge\",\"edge\":\"left\",\"autofocus\":\"search\"}\n---\n```mix\nroot: {widget: \"column\", children: [\"search\"]}\nsearch: {widget: \"field\", value: \"\", on_change: \"filter\"}\n```\n";
    let mut rt = runtime(source, (360, 600));
    assert_eq!(autofocus(&rt.program.content.tree), Some("search"));
    let ctx = rt.context().clone();
    assert!(rt.program.focus_field(&ctx, "search"));
    assert!(!rt.program.focus_field(&ctx, "missing"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    rt.set_message_handler(move |message: &SceneMessage| {
        if let Some((_, handler, body)) = event_of(message) {
            sink.lock().unwrap().push((handler.to_owned(), body));
        }
    });
    rt.tick();
    rt.queue_input(Input::Focused(true));
    rt.queue_input(key(41, 0x0066, Some("f"), true, false));
    rt.queue_input(key(41, 0x0066, Some("f"), false, false));
    for _ in 0..3 {
        rt.tick();
    }
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|(handler, body)| handler == "filter" && body["value"] == json!("f")),
        "{seen:?}"
    );
}

/// A right click on a list row emits the list's `on_context` with the row
/// item; a left click still emits `on_click`. A row with no `on_context`
/// sends nothing on a right click.
#[test]
fn a_right_click_on_a_row_emits_the_lists_context_event_with_its_item() {
    let source = "---\nscene: 1\nname: pager\ncitizen: test\nwindow: {\"kind\":\"edge\",\"edge\":\"bottom\",\"h\":40}\n---\n```mix\nroot: {widget: \"row\", children: [\"pager\"]}\npager: {widget: \"list\", flow: \"horizontal\", rows: [{id: \"ws_1\", cells: [], index: 1}, {id: \"ws_2\", cells: [], index: 2}], row: \"ws_btn\", row_height: 30, on_click: \"ws\", on_context: \"ws_rename\"}\nws_btn: {widget: \"row\", height: 30, min_width: 30, children: [\"ws_label\"]}\nws_label: {widget: \"text\", text: \"= $item.id\"}\n```\n";
    let mut rt = runtime(source, (300, 40));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    rt.set_message_handler(move |message: &SceneMessage| {
        if let Some((_, handler, body)) = event_of(message) {
            sink.lock().unwrap().push((handler.to_owned(), body));
        }
    });
    let second = row_of(&rt, "pager", "ws_2").center();
    rt.queue_input(Input::PointerMoved {
        x: second.x,
        y: second.y,
    });
    rt.tick();
    // BTN_RIGHT (0x111), then BTN_LEFT (0x110).
    for code in [0x111, 0x110] {
        rt.queue_input(Input::Button {
            code,
            pressed: true,
        });
        rt.tick();
        rt.queue_input(Input::Button {
            code,
            pressed: false,
        });
        rt.tick();
    }
    let seen = seen.lock().unwrap();
    let context: Vec<_> = seen.iter().filter(|(h, _)| h == "ws_rename").collect();
    assert_eq!(context.len(), 1, "{seen:?}");
    assert_eq!(context[0].1["kind"], "context");
    assert_eq!(context[0].1["item"]["index"].as_f64(), Some(2.0));
    assert_eq!(context[0].1["node"], "pager");
    assert_eq!(
        seen.iter().filter(|(h, _)| h == "ws").count(),
        1,
        "the left click still clicks: {seen:?}"
    );
}

/// A button's own `on_context` answers a right click; a button without one
/// sends nothing on a right click.
#[test]
fn a_right_click_on_a_button_emits_its_context_event() {
    let source = "---\nscene: 1\nname: buttons\ncitizen: test\nwindow: {\"kind\":\"edge\",\"edge\":\"bottom\",\"h\":40}\n---\n```mix\nroot: {widget: \"row\", children: [\"a\", \"b\"]}\na: {widget: \"button\", label: \"A\", width: 80, on_click: \"a.click\", on_context: \"a.menu\"}\nb: {widget: \"button\", label: \"B\", width: 80, on_click: \"b.click\"}\n```\n";
    let mut rt = runtime(source, (300, 40));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    rt.set_message_handler(move |message: &SceneMessage| {
        if let Some((_, handler, body)) = event_of(message) {
            sink.lock().unwrap().push((handler.to_owned(), body));
        }
    });
    for id in ["a", "b"] {
        let at = rect_of(&rt, id).center();
        rt.queue_input(Input::PointerMoved { x: at.x, y: at.y });
        rt.tick();
        for pressed in [true, false] {
            rt.queue_input(Input::Button {
                code: 0x111,
                pressed,
            });
            rt.tick();
        }
    }
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        ["a.menu"],
        "only A has a context handler, and a right click is not a click: {seen:?}"
    );
    assert_eq!(seen[0].1["kind"], "context");
}

#[test]
fn every_calendar_node_has_visible_non_zero_bounds_including_the_footer() {
    for (width, height) in [(360, 600), (380, 812), (360, 600)] {
        let rt = runtime(
            include_str!("../../tests/fixtures/calendar.scene.mix"),
            (width, height),
        );
        for id in rt.program.content.tree.nodes.keys() {
            let bounds = rect_of(&rt, id);
            assert!(
                bounds.width() > 0.0 && bounds.height() > 0.0,
                "{id}: {bounds:?} at {width}x{height}"
            );
            assert!(
                bounds.left() >= 0.0
                    && bounds.top() >= 0.0
                    && bounds.right() <= width as f32 + 0.1
                    && bounds.bottom() <= height as f32 + 0.1,
                "calendar node {id} outside viewport: {bounds:?} at {width}x{height}"
            );
        }
        let footer = rect_of(&rt, "cal_open");
        let fill = rect_of(&rt, "cal_fill");
        let last_week = rect_of(&rt, "wk_5");
        assert!(last_week.bottom() <= fill.top() + 0.1);
        assert!(fill.bottom() <= footer.top() + 0.1);
        assert!(
            (footer.bottom() - (height as f32 - 18.0)).abs() < 0.6,
            "footer must stay above the bottom padding: {footer:?}"
        );
    }
}

#[test]
fn scene_buttons_take_their_design_cell_for_every_tone_and_state() {
    let theme = embedded(true);
    let tree = scene::resolve(&scene::parse("---\nscene: 1\nname: buttons\ncitizen: c\n---\n```mix\nroot: {widget: \"row\", children: [\"plain\", \"primary\", \"danger\"]}\nplain: {widget: \"button\", label: \"P\"}\nprimary: {widget: \"button\", label: \"P\", tone: \"primary\"}\ndanger: {widget: \"button\", label: \"D\", tone: \"danger\"}\n```\n").unwrap()).unwrap();
    let tables = theme.design().tables();
    let cell = |id: &str, state| {
        tables
            .button
            .cell(button_key(&tree.nodes[id], state))
            .pair
            .surface
    };
    for state in [
        InteractionState::Resting,
        InteractionState::Hovered,
        InteractionState::Pressed,
    ] {
        assert_ne!(cell("plain", state), cell("primary", state), "{state:?}");
        assert_ne!(cell("primary", state), cell("danger", state), "{state:?}");
    }
    assert_ne!(
        cell("primary", InteractionState::Resting),
        cell("primary", InteractionState::Hovered)
    );
}

#[test]
fn scene_colours_parse_as_quoin_parses_them() {
    assert_eq!(hex("#fff"), Some(Color32::from_rgb(255, 255, 255)));
    assert_eq!(hex("#20242d"), Some(Color32::from_rgb(0x20, 0x24, 0x2d)));
    assert_eq!(
        hex("#00000080").map(|c| c.to_srgba_unmultiplied()[3]),
        Some(128)
    );
    for bad in ["", "fff", "#ff", "#gggggg", "#ffffffff0", "#ééé"] {
        assert_eq!(hex(bad), None, "{bad}");
    }
    assert_eq!(hex_of(Color32::from_rgb(0x20, 0x23, 0x26)), "#202326ff");
}

#[test]
fn events_carry_the_quoin_payload_and_only_bound_handlers_fire() {
    let tree = scene::resolve(&scene::parse(scene::fixtures::CONFORMANCE).unwrap()).unwrap();
    let button = Arc::new(Route::new(&tree, "button", &tree.nodes["button"]));
    let click = SceneMessage::Click(Arc::clone(&button));
    let (citizen, handler, body) = event_of(&click).unwrap();
    assert_eq!((citizen, handler), ("conformance-citizen", "go"));
    assert_eq!(
        body,
        json!({"scene":"conformance","node":"button","kind":"click"})
    );
    assert!(event_of(&SceneMessage::Toggle(button, "k".into(), true)).is_none());
    let field = Arc::new(Route::new(&tree, "field", &tree.nodes["field"]));
    let submit = SceneMessage::Submit(Arc::clone(&field), "typed".into());
    let (_, handler, body) = event_of(&submit).unwrap();
    assert_eq!((handler, body["value"].as_str()), ("submit", Some("typed")));
    let mut row = Route::new(&tree, "list", &tree.nodes["list"]);
    row.item = Some(json!({"id":"1","cells":["one"]}));
    let row_click = SceneMessage::Click(Arc::new(row));
    let (_, handler, body) = event_of(&row_click).unwrap();
    assert_eq!(handler, "select");
    assert_eq!(
        body,
        json!({"scene":"conformance","node":"list","kind":"click","item":{"id":"1","cells":["one"]}})
    );
}

#[test]
fn edges_draw_on_the_dark_panel_pair_and_dialogs_on_the_base_pair() {
    let source = scene::fixtures::CONFORMANCE;
    let edge = SceneUi::new(content(source, None, false));
    let dialog = SceneUi::new(content(source, Some(Frame { title: "t".into() }), true));
    let dark = crate::appearance::page(&embedded(false), true);
    assert_eq!(
        edge.page(),
        colour(dark.secondary.surface),
        "edge: the secondary pair, dark"
    );
    assert_eq!(
        dialog.page(),
        colour(crate::appearance::page(&embedded(true), true).base.surface)
    );
    assert_eq!(hex(&hex_of(edge.page())), Some(edge.page()));
}

#[test]
fn escape_routes_to_the_dialog_or_the_edge() {
    for (dialog, expected) in [(true, "close"), (false, "edge")] {
        let source = scene::fixtures::CONFORMANCE;
        let mut rt =
            UiRuntime::headless(SceneUi::new(content(source, None, dialog)), (400, 300), 1.0);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        rt.set_message_handler(move |message: &SceneMessage| {
            let name = match message {
                SceneMessage::Close => "close",
                SceneMessage::EscapeEdge => "edge",
                _ => return,
            };
            sink.lock().unwrap().push(name);
        });
        rt.tick();
        rt.queue_input(key(9, 0xff1b, None, true, false));
        rt.tick();
        assert_eq!(*seen.lock().unwrap(), vec![expected], "dialog={dialog}");
    }
}

#[test]
fn spacers_and_explicit_lengths_keep_their_sizes() {
    let rt = runtime(
        "---\nscene: 1\nname: sizes\ncitizen: c\n---\n```mix\nroot: {widget: \"row\", children: [\"fixed\", \"gap\", \"box\", \"grow\", \"tail\"]}\nfixed: {widget: \"text\", text: \"x\", width: 50}\ngap: {widget: \"spacer\", size: 12}\nbox: {widget: \"image\", src: \"\", w: 40, h: 30}\ngrow: {widget: \"spacer\"}\ntail: {widget: \"text\", text: \"end\"}\n```\n",
        (400, 100),
    );
    assert_eq!(rect_of(&rt, "fixed").width(), 50.0);
    let gap = rect_of(&rt, "gap");
    assert_eq!((gap.width(), gap.height()), (12.0, 12.0));
    let boxed = rect_of(&rt, "box");
    assert_eq!((boxed.width(), boxed.height()), (40.0, 30.0));
    assert!(
        rect_of(&rt, "grow").width() > 200.0,
        "an unsized spacer takes the rest"
    );
    assert!(
        (rect_of(&rt, "tail").right() - 400.0).abs() < 0.6,
        "pushed to the end"
    );
}

#[test]
fn autofocus_only_lands_on_a_reachable_visible_field() {
    let hidden = content(
        "---\nscene: 1\nname: lists\ncitizen: c\nwindow: {\"kind\":\"edge\",\"edge\":\"left\",\"autofocus\":\"search\"}\n---\n```mix\nroot: {widget: \"column\", children: [\"wrap\"]}\nwrap: {widget: \"column\", hidden: true, children: [\"search\"]}\nsearch: {widget: \"field\", value: \"\"}\n```\n",
        None,
        false,
    );
    assert_eq!(autofocus(&hidden.tree), None);
    let not_a_field = content(
        "---\nscene: 1\nname: lists\ncitizen: c\nwindow: {\"kind\":\"edge\",\"edge\":\"left\",\"autofocus\":\"search\"}\n---\n```mix\nroot: {widget: \"column\", children: [\"search\"]}\nsearch: {widget: \"text\", text: \"x\"}\n```\n",
        None,
        false,
    );
    assert_eq!(autofocus(&not_a_field.tree), None);
}

#[test]
fn a_list_shows_at_most_max_rows_before_it_scrolls() {
    let rows: Vec<String> = (0..12)
        .map(|i| format!("{{id: \"r{i}\", cells: [\"row {i}\"]}}"))
        .collect();
    let source = format!(
        "---\nscene: 1\nname: lists\ncitizen: c\n---\n```mix\nroot: {{widget: \"column\", children: [\"list\"]}}\nlist: {{widget: \"list\", row: \"item\", row_height: 20, max_rows: 5, rows: [{}]}}\nitem: {{widget: \"row\", children: [\"label\"]}}\nlabel: {{widget: \"text\", text: \"{{0}}\"}}\n```\n",
        rows.join(", ")
    );
    let rt = runtime(&source, (300, 600));
    let list = rect_of(&rt, "list");
    assert_eq!(list.height(), 100.0, "five rows of twenty");
    let boxed = &rt.program.layout().lists["list"];
    assert!(
        boxed.content_height >= 240.0,
        "twelve rows to scroll through: {}",
        boxed.content_height
    );
    assert_eq!(row_of(&rt, "list", "r0").height(), 20.0);
    assert!(
        row_of(&rt, "list", "r11").top() > list.bottom(),
        "past the box, until scrolled"
    );
}

/// A scrolled list that shrinks keeps its surviving rows in view: the
/// offset is clamped to the new content on the next pass, without a wheel.
#[test]
fn a_shrunk_list_pulls_its_scroll_back_into_its_content() {
    let source = |count: usize| {
        let rows: Vec<String> = (0..count)
            .map(|i| format!("{{id: \"r{i}\", cells: [\"row {i}\"]}}"))
            .collect();
        format!(
            "---\nscene: 1\nname: lists\ncitizen: c\n---\n```mix\nroot: {{widget: \"column\", children: [\"list\"]}}\nlist: {{widget: \"list\", row: \"item\", row_height: 20, max_rows: 5, rows: [{}]}}\nitem: {{widget: \"row\", children: [\"label\"]}}\nlabel: {{widget: \"text\", text: \"{{0}}\"}}\n```\n",
            rows.join(", ")
        )
    };
    let mut rt = runtime(&source(12), (300, 600));
    rt.program.scroll.insert("list".into(), 140.0);
    rt.request_redraw();
    rt.tick();
    assert_eq!(rt.program.scrolled("list"), 140.0, "inside twelve rows");
    rt.queue_message(SceneMessage::Replace(content(&source(1), None, false)));
    rt.tick();
    assert_eq!(rt.program.scrolled("list"), 0.0, "one row cannot scroll");
    rt.program.scroll.insert("gone".into(), 10.0);
    rt.request_redraw();
    rt.tick();
    assert!(
        !rt.program.scroll.contains_key("gone"),
        "a list's offset leaves with it"
    );
}

#[test]
fn a_dialog_frame_reports_its_close_button_and_puts_the_page_below_the_title_bar() {
    let rt = {
        let mut rt = UiRuntime::headless(
            SceneUi::new(content(
                scene::fixtures::CONFORMANCE,
                Some(Frame {
                    title: "Title".into(),
                }),
                true,
            )),
            (720, 520),
            1.0,
        );
        rt.tick();
        rt
    };
    let layout = rt.program.layout();
    let close = layout.close.expect("a framed dialog has a close button");
    assert_eq!((close.width(), close.height()), (28.0, 24.0));
    assert!((close.right() - (720.0 - 11.0)).abs() < 0.1);
    assert!(layout.placed.iter().all(|node| node.rect.top() >= TITLEBAR));
    assert!(rect_of(&rt, "root").top() >= TITLEBAR);
    let _ = PreparedLists::new();
}
