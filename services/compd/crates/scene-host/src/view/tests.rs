// SPDX-License-Identifier: MIT OR Apache-2.0
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
    Arc::new(Content { tree, lists, revision: 1, frame, dialog })
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

/// The placed text node `id` inside list `list`'s row `item`.
fn text_in_row(rt: &UiRuntime<SceneUi>, list: &str, item: &str, id: &str) -> egui::Rect {
    rt.program
        .layout()
        .placed
        .iter()
        .find(|placed| {
            placed.id == id && placed.source == flex::Source::Row { list: list.into(), item: item.into() }
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
    let route = Arc::new(Route::new(&content.tree, "entry", &content.tree.nodes["entry"]));
    ui::UiProgram::update(&mut ui, SceneMessage::Input(Arc::clone(&route), "/root/entry".into(), "unsaved edit".into()));
    ui::UiProgram::update(&mut ui, SceneMessage::Toggle(route, "/root/enabled".into(), true));
    ui.apply_appearance(crate::appearance::fixture("light", 1.0));
    let light = ui.page();
    ui::UiProgram::update(&mut ui, SceneMessage::Appearance(crate::appearance::fixture("dark", 1.5)));
    assert!(Arc::ptr_eq(&content, &ui.content));
    assert_eq!(ui.edits["/root/entry"], "unsaved edit");
    assert!(ui.toggles["/root/enabled"]);
    assert_ne!(ui.page(), light);
    assert!(ui.text_styles().ui.size > TEXT_SIZE, "text scale 1.5 applies");
}

#[test]
fn panel_has_square_pager_two_clock_lines_and_full_task_hit_targets() {
    let rt = runtime(include_str!("../../tests/fixtures/panel-render.scene.mix"), (1536, 52));
    for id in ["1", "2", "3", "4"] {
        let cell = row_of(&rt, "pager", id);
        assert!((cell.width() - cell.height()).abs() < 0.1, "pager {id}: {cell:?}");
        assert!((cell.height() - 30.0).abs() < 0.1, "pager {id}: {cell:?}");
    }
    let time = rect_of(&rt, "clock_time");
    let date = rect_of(&rt, "clock_date");
    assert!(time.height() >= 22.0 && date.height() >= 13.0, "clock: {time:?}, {date:?}");
    assert!(date.top() >= time.bottom() - 0.1 && date.bottom() <= 48.0, "clock: {time:?}, {date:?}");
    let task = row_of(&rt, "tasks", "task");
    assert!(task.width() >= 218.0 && task.height() >= 40.0, "task: {task:?}");
    let peek = rect_of(&rt, "peek_i");
    assert_eq!((peek.width(), peek.height()), (20.0, 20.0));
}

#[test]
fn panel_pager_digits_sit_at_the_centre_of_their_hit_targets() {
    let rt = runtime(include_str!("../../tests/fixtures/panel-render.scene.mix"), (1536, 52));
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
        assert!((digit.center().x - cell.center().x).abs() < 0.6, "pager {id}: digit={digit:?}, cell={cell:?}");
        assert!((digit.center().y - cell.center().y).abs() < 0.6, "pager {id}: digit={digit:?}, cell={cell:?}");
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
        assert!(((a.left() + b.right()) / 2.0 - row.center().x).abs() < 0.1, "row={row:?}, a={a:?}, b={b:?}");
        assert!(((c.top() + d.bottom()) / 2.0 - column.center().y).abs() < 0.1, "column={column:?}, c={c:?}, d={d:?}");
        assert!((b.left() - a.right() - 6.0).abs() < 0.1);
        assert!((d.top() - c.bottom() - 6.0).abs() < 0.1);
        // min_width holds against the short content; min = max fixes the column.
        assert!((row.width() - 60.0).abs() < 0.1, "row={row:?}");
        assert!((column.width() - 60.0).abs() < 0.1 && (column.height() - 60.0).abs() < 0.1, "column={column:?}");
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
    rt.queue_input(key(0x0066, Some("f"), true, false));
    rt.queue_input(key(0x0066, Some("f"), false, false));
    for _ in 0..3 {
        rt.tick();
    }
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|(handler, body)| handler == "filter" && body["value"] == json!("f")),
        "{seen:?}"
    );
}

#[test]
fn every_calendar_node_has_visible_non_zero_bounds_including_the_footer() {
    for (width, height) in [(360, 600), (380, 812), (360, 600)] {
        let rt = runtime(include_str!("../../tests/fixtures/calendar.scene.mix"), (width, height));
        for id in rt.program.content.tree.nodes.keys() {
            let bounds = rect_of(&rt, id);
            assert!(bounds.width() > 0.0 && bounds.height() > 0.0, "{id}: {bounds:?} at {width}x{height}");
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
    let tree = scene::resolve(&scene::parse("---\nscene: 1\nname: b\ncitizen: c\n---\n```mix\nroot: {widget: \"row\", children: [\"plain\", \"primary\", \"danger\"]}\nplain: {widget: \"button\", label: \"P\"}\nprimary: {widget: \"button\", label: \"P\", tone: \"primary\"}\ndanger: {widget: \"button\", label: \"D\", tone: \"danger\"}\n```\n").unwrap()).unwrap();
    let tables = theme.design().tables();
    let cell = |id: &str, state| tables.button.cell(button_key(&tree.nodes[id], state)).pair.surface;
    for state in [InteractionState::Resting, InteractionState::Hovered, InteractionState::Pressed] {
        assert_ne!(cell("plain", state), cell("primary", state), "{state:?}");
        assert_ne!(cell("primary", state), cell("danger", state), "{state:?}");
    }
    assert_ne!(cell("primary", InteractionState::Resting), cell("primary", InteractionState::Hovered));
}

#[test]
fn scene_colours_parse_as_quoin_parses_them() {
    assert_eq!(hex("#fff"), Some(Color32::from_rgb(255, 255, 255)));
    assert_eq!(hex("#20242d"), Some(Color32::from_rgb(0x20, 0x24, 0x2d)));
    assert_eq!(hex("#00000080").map(|c| c.to_srgba_unmultiplied()[3]), Some(128));
    for bad in ["", "fff", "#ff", "#gggggg", "#ffffffff0", "#ééé"] {
        assert_eq!(hex(bad), None, "{bad}");
    }
    assert_eq!(hex_of(Color32::from_rgb(0x20, 0x23, 0x26)), "#202326ff");
}

#[test]
fn events_carry_the_quoin_payload_and_only_bound_handlers_fire() {
    let tree = scene::resolve(&scene::parse(scene::fixtures::CONFORMANCE).unwrap()).unwrap();
    let button = Arc::new(Route::new(&tree, "button", &tree.nodes["button"]));
    let (citizen, handler, body) = event_of(&SceneMessage::Click(Arc::clone(&button))).unwrap();
    assert_eq!((citizen, handler), ("conformance-citizen", "go"));
    assert_eq!(body, json!({"scene":"conformance","node":"button","kind":"click"}));
    assert!(event_of(&SceneMessage::Toggle(button, "k".into(), true)).is_none());
    let field = Arc::new(Route::new(&tree, "field", &tree.nodes["field"]));
    let (_, handler, body) = event_of(&SceneMessage::Submit(Arc::clone(&field), "typed".into())).unwrap();
    assert_eq!((handler, body["value"].as_str()), ("submit", Some("typed")));
    let mut row = Route::new(&tree, "list", &tree.nodes["list"]);
    row.item = Some(json!({"id":"1","cells":["one"]}));
    let (_, handler, body) = event_of(&SceneMessage::Click(Arc::new(row))).unwrap();
    assert_eq!(handler, "select");
    assert_eq!(body, json!({"scene":"conformance","node":"list","kind":"click","item":{"id":"1","cells":["one"]}}));
}

#[test]
fn edges_draw_on_the_dark_panel_pair_and_dialogs_on_the_base_pair() {
    let source = scene::fixtures::CONFORMANCE;
    let edge = SceneUi::new(content(source, None, false));
    let dialog = SceneUi::new(content(source, Some(Frame { title: "t".into() }), true));
    let dark = crate::appearance::page(&embedded(false), true);
    assert_eq!(edge.page(), colour(dark.secondary.surface), "edge: the secondary pair, dark");
    assert_eq!(dialog.page(), colour(crate::appearance::page(&embedded(true), true).base.surface));
    assert_eq!(hex(&hex_of(edge.page())), Some(edge.page()));
}

#[test]
fn escape_routes_to_the_dialog_or_the_edge() {
    for (dialog, expected) in [(true, "close"), (false, "edge")] {
        let source = scene::fixtures::CONFORMANCE;
        let mut rt = UiRuntime::headless(SceneUi::new(content(source, None, dialog)), (400, 300), 1.0);
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
        rt.queue_input(key(0xff1b, None, true, false));
        rt.tick();
        assert_eq!(*seen.lock().unwrap(), vec![expected], "dialog={dialog}");
    }
}

#[test]
fn spacers_and_explicit_lengths_keep_their_sizes() {
    let rt = runtime(
        "---\nscene: 1\nname: sizes\ncitizen: c\n---\n```mix\nroot: {widget: \"row\", children: [\"fixed\", \"gap\", \"box\", \"grow\", \"tail\"]}\nfixed: {widget: \"text\", text: \"x\", width: 50}\ngap: {widget: \"spacer\", size: 12}\nbox: {widget: \"column\", width: 40, height: 30, children: []}\ngrow: {widget: \"spacer\"}\ntail: {widget: \"text\", text: \"end\"}\n```\n",
        (400, 100),
    );
    assert_eq!(rect_of(&rt, "fixed").width(), 50.0);
    let gap = rect_of(&rt, "gap");
    assert_eq!((gap.width(), gap.height()), (12.0, 12.0));
    let boxed = rect_of(&rt, "box");
    assert_eq!((boxed.width(), boxed.height()), (40.0, 30.0));
    assert!(rect_of(&rt, "grow").width() > 200.0, "an unsized spacer takes the rest");
    assert!((rect_of(&rt, "tail").right() - 400.0).abs() < 0.6, "pushed to the end");
}

#[test]
fn autofocus_only_lands_on_a_reachable_visible_field() {
    let hidden = content(
        "---\nscene: 1\nname: l\ncitizen: c\nwindow: {\"kind\":\"edge\",\"edge\":\"left\",\"autofocus\":\"search\"}\n---\n```mix\nroot: {widget: \"column\", children: [\"wrap\"]}\nwrap: {widget: \"column\", hidden: true, children: [\"search\"]}\nsearch: {widget: \"field\"}\n```\n",
        None,
        false,
    );
    assert_eq!(autofocus(&hidden.tree), None);
    let not_a_field = content(
        "---\nscene: 1\nname: l\ncitizen: c\nwindow: {\"kind\":\"edge\",\"edge\":\"left\",\"autofocus\":\"search\"}\n---\n```mix\nroot: {widget: \"column\", children: [\"search\"]}\nsearch: {widget: \"text\", text: \"x\"}\n```\n",
        None,
        false,
    );
    assert_eq!(autofocus(&not_a_field.tree), None);
}

#[test]
fn a_list_shows_at_most_max_rows_before_it_scrolls() {
    let rows: Vec<String> = (0..12).map(|i| format!("{{id: \"r{i}\", cells: [\"row {i}\"]}}")).collect();
    let source = format!(
        "---\nscene: 1\nname: l\ncitizen: c\n---\n```mix\nroot: {{widget: \"column\", children: [\"list\"]}}\nlist: {{widget: \"list\", row: \"item\", row_height: 20, max_rows: 5, rows: [{}]}}\nitem: {{widget: \"row\", children: [\"label\"]}}\nlabel: {{widget: \"text\", text: \"{{0}}\"}}\n```\n",
        rows.join(", ")
    );
    let rt = runtime(&source, (300, 600));
    let list = rect_of(&rt, "list");
    assert_eq!(list.height(), 100.0, "five rows of twenty");
    let boxed = &rt.program.layout().lists["list"];
    assert!(boxed.content_height >= 240.0, "twelve rows to scroll through: {}", boxed.content_height);
    assert_eq!(row_of(&rt, "list", "r0").height(), 20.0);
    assert!(row_of(&rt, "list", "r11").top() > list.bottom(), "past the box, until scrolled");
}

#[test]
fn a_dialog_frame_reports_its_close_button_and_puts_the_page_below_the_title_bar() {
    let rt = {
        let mut rt = UiRuntime::headless(
            SceneUi::new(content(scene::fixtures::CONFORMANCE, Some(Frame { title: "Title".into() }), true)),
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
    assert_eq!(layout.body.top(), TITLEBAR);
    assert!(rect_of(&rt, "root").top() >= TITLEBAR);
    let _ = PreparedLists::new();
}
