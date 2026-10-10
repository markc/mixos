// SPDX-License-Identifier: MIT OR Apache-2.0
//! The drive layer driven the way an app's shell drives it: each request is
//! made inside a frame and answered then or from a later frame, against the
//! fixture's real title bar and menus, a text field and a scrolled list.

#[path = "support/fixture.rs"]
mod fixture;

use design::{Mode, Scheme};
use egui::{Color32, ColorImage, Context, Event, FullOutput, ViewportCommand, ViewportId};
use egui_kittest::Harness;
use fixture::Fixture;
use serde_json::{Value, json};
use std::sync::Arc;
use toolkit::drive::{self, Answer};
use toolkit::{Icon, Strings, menu, titlebar};

/// The fixture's window plus the Bus side of a shell.
#[derive(Default)]
struct App {
    fixture: Fixture,
    name: String,
    /// Clicks on the Count button.
    count: u32,
    /// The Level slider (resets to 150 on a double click; a click at its
    /// centre sets 100).
    level: f64,
    inbox: Vec<(u64, &'static str, Value)>,
    answers: Vec<Answer>,
    next: u64,
    /// Answer everything as an app does when it closes, next frame.
    finish: bool,
    /// The runtime directory captures go under.
    runtime: Option<tempfile::TempDir>,
}

/// Every viewport command the window sent.
#[derive(Default)]
struct Sent(Vec<ViewportCommand>);

impl egui::Plugin for Sent {
    fn debug_name(&self) -> &'static str {
        "sent"
    }

    fn output_hook(&mut self, _ctx: &Context, output: &mut FullOutput) {
        for viewport in output.viewport_output.values() {
            self.0.extend(viewport.commands.iter().cloned());
        }
    }
}

fn window() -> Harness<'static, App> {
    window_with(Harness::builder())
}

/// Passes a 60th of a second apart, as a live window draws: a click, a
/// scroll and a click then fit in a double click's half second.
fn live_window() -> Harness<'static, App> {
    window_with(
        Harness::builder()
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(240),
    )
}

fn window_with(builder: egui_kittest::HarnessBuilder<App>) -> Harness<'static, App> {
    let registry = fixture::registry();
    let strings = Strings::new(fixture::FTL);
    let theme = fixture::theme(Scheme::Pro, Mode::Light);
    let stroke = toolkit::icons::stroke_width(&theme);
    let builder = builder.with_size(egui::vec2(900.0, 500.0));
    let mut harness = builder.build_ui_state(
        move |ui, app: &mut App| {
            for (id, verb, args) in std::mem::take(&mut app.inbox) {
                app.answers
                    .extend(drive::request(ui.ctx(), id, verb, &args));
            }
            app.answers.extend(drive::logic(ui.ctx()));
            if std::mem::take(&mut app.finish) {
                app.answers.extend(drive::finish(ui.ctx()));
            }
            let mut fired = registry.shortcuts(ui.ctx(), &app.fixture);
            fired.extend(titlebar::show(
                ui,
                fixture::TITLE,
                Some(Icon::Square),
                stroke,
                &registry,
                &app.fixture,
                &strings,
            ));
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.name).hint_text("Name"));
                if ui.button("Count").clicked() {
                    app.count += 1;
                }
                ui.add(
                    toolkit::slider::Slider::new(&mut app.level, 0.0..=200.0)
                        .reset_to(150.0)
                        .wheel(1.0)
                        .label("Level"),
                );
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for row in 0..60 {
                            ui.label(format!("Row {row}"));
                        }
                    });
            });
            titlebar::edges(ui);
            for id in fired {
                let _ = registry.execute(id, &mut app.fixture);
            }
        },
        App::default(),
    );
    toolkit::install(&harness.ctx, &theme);
    let runtime = tempfile::tempdir().unwrap();
    let options = drive::Options {
        runtime_dir: Some(runtime.path().to_owned()),
        ..drive::Options::default()
    };
    drive::install_with(&harness.ctx, "demo", options);
    harness.state_mut().runtime = Some(runtime);
    harness.ctx.add_plugin(Sent::default());
    harness.run_steps(2);
    harness
}

/// Send `verb` from inside the next frame and step until it is answered.
fn call(h: &mut Harness<'_, App>, verb: &'static str, args: Value) -> Answer {
    let app = h.state_mut();
    app.next += 1;
    let id = app.next;
    app.inbox.push((id, verb, args));
    for _ in 0..60 {
        h.step();
        if let Some(at) = h.state().answers.iter().position(|a| a.id == id) {
            return h.state_mut().answers.remove(at);
        }
    }
    panic!("{verb} was never answered");
}

fn ok(h: &mut Harness<'_, App>, verb: &'static str, args: Value) -> Value {
    let answer = call(h, verb, args);
    assert_eq!(answer.rc, 0, "{verb}: {}", answer.body);
    answer.body
}

fn sent(h: &Harness<'_, App>) -> Vec<ViewportCommand> {
    h.ctx
        .with_plugin(|s: &mut Sent| s.0.clone())
        .unwrap_or_default()
}

fn node<'a>(tree: &'a Value, label: &str) -> &'a Value {
    tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["label"] == label)
        .unwrap_or_else(|| panic!("no {label}"))
}

/// The live VT4 bug: a compositor draws an unfocused window once a second,
/// so the drive's passes are a second apart. A press and release in
/// separate passes was then a one-second long press, which egui does not
/// count as a click (over 0.8 s): every button ignored ui.click, while menus
/// (which open on press) still worked.
#[test]
fn a_click_works_when_passes_are_a_second_apart() {
    let mut h = window_with(Harness::builder().with_step_dt(1.0));
    ok(&mut h, "ui.click", json!({"label":"Count"}));
    h.run();
    assert_eq!(h.state().count, 1, "a button clicks at one pass a second");
    ok(&mut h, "ui.click", json!({"label":"Count","double":true}));
    h.run();
    assert_eq!(
        h.state().count,
        2,
        "a double click is one click of a plain button"
    );
    let opened = ok(&mut h, "ui.click", json!({"label":"File"}));
    assert_eq!(opened["menu"]["menu"], "File", "a menu still opens");
}

/// `trace: true` answers each pass: its input events, its time and what
/// egui hit-tested, so a click that does nothing can be explained.
#[test]
fn a_traced_click_reports_each_pass() {
    let mut h = window_with(Harness::builder().with_step_dt(1.0));
    let answer = ok(&mut h, "ui.click", json!({"label":"Count","trace":true}));
    let passes = answer["passes"].as_array().expect("passes");
    assert!(passes.len() >= 2, "{answer}");
    let has = |p: &Value, text: &str| {
        p["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains(text))
    };
    let clicking = passes
        .iter()
        .find(|p| has(p, "pressed: true"))
        .expect("the press");
    assert!(
        has(clicking, "pressed: false"),
        "the release shares the press's pass: {clicking}"
    );
    assert!(
        clicking["clicked"].is_string(),
        "egui clicked a widget: {clicking}"
    );
    assert!(
        passes
            .windows(2)
            .all(|w| w[1]["time"].as_f64() > w[0]["time"].as_f64()),
        "times rise"
    );
    assert!(
        call(&mut h, "ui.click", json!({"label":"Count","trace":"yes"})).rc != 0,
        "trace must be a boolean"
    );
    let plain = ok(&mut h, "ui.click", json!({"label":"Count"}));
    assert!(plain.get("passes").is_none(), "only on request");
}

/// sol round 2, 8: click, scroll, click on one spot of a slider within half
/// a second is no double click; click, click is.
#[test]
fn a_scroll_between_two_clicks_is_no_double_click() {
    let mut h = live_window();
    h.state_mut().level = 20.0;
    let tree = ok(&mut h, "ui.tree", json!({"label":"Level","exact":true}));
    let rect = &tree["nodes"][0]["rect"];
    let (x, y) = (
        (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.0,
        (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.0,
    );
    // Three queued requests, run one after another by the drive layer.
    let app = h.state_mut();
    app.inbox.push((101, "ui.click", json!({"label":"Level"})));
    app.inbox
        .push((102, "ui.scroll", json!({"x":x,"y":y,"dx":0.0,"dy":40.0})));
    app.inbox.push((103, "ui.click", json!({"label":"Level"})));
    for _ in 0..120 {
        h.step();
        if h.state().answers.iter().any(|a| a.id == 103) {
            break;
        }
    }
    let answered: Vec<_> = h.state().answers.iter().map(|a| (a.id, a.rc)).collect();
    assert!(
        [101, 102, 103]
            .iter()
            .all(|id| answered.contains(&(*id, 0))),
        "{answered:?}"
    );
    assert_eq!(
        h.state().level,
        100.0,
        "the scroll between the clicks cancels the reset: the second click only moves it"
    );
    // The control: two clicks alone do reset (after a pause, so the last
    // click above does not pair with the first below).
    h.run_steps(60);
    h.state_mut().answers.clear();
    h.state_mut().level = 20.0;
    let app = h.state_mut();
    app.inbox.push((104, "ui.click", json!({"label":"Level"})));
    app.inbox.push((105, "ui.click", json!({"label":"Level"})));
    for _ in 0..120 {
        h.step();
        if h.state().answers.iter().any(|a| a.id == 105) {
            break;
        }
    }
    assert_eq!(
        h.state().level,
        150.0,
        "click, click within half a second resets"
    );
}

#[test]
fn the_tree_lists_widgets_with_their_state() {
    let mut h = window();
    let tree = ok(&mut h, "ui.tree", json!({}));
    let file = node(&tree, "File");
    assert_eq!(file["enabled"], true);
    assert!(file["rect"].as_array().is_some_and(|r| r.len() == 4));
    let name = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["placeholder"] == "Name")
        .expect("the text field");
    assert_eq!(name["focused"], false);
    assert!(tree["nodes"][0]["depth"] == 0, "depth first from the root");
    let filtered = ok(&mut h, "ui.tree", json!({"label":"fil"}));
    assert!(
        filtered["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["label"].as_str().unwrap().to_lowercase().contains("fil"))
    );
    assert!(!filtered["nodes"].as_array().unwrap().is_empty());
}

#[test]
fn a_click_on_a_menu_title_opens_it_and_keys_navigate() {
    let mut h = window();
    let opened = ok(&mut h, "ui.click", json!({"label":"File"}));
    assert_eq!(opened["menu"]["menu"], "File", "{opened}");
    assert_eq!(opened["target"]["label"], "File");
    assert_eq!(
        menu::current(&h.ctx).map(|c| c.menu).as_deref(),
        Some("File")
    );
    let down = ok(&mut h, "ui.key", json!({"key":"ArrowDown"}));
    assert_eq!(down["menu"]["path"], json!(["New…"]));
    let ran = ok(&mut h, "ui.key", json!({"key":"Enter"}));
    assert_eq!(ran["menu"], Value::Null, "running closes the menus");
    assert_eq!(h.state().fixture.ran, ["file.new"]);
    let menu = ok(&mut h, "ui.menu", json!({}));
    assert_eq!(menu["menu"], Value::Null);
}

#[test]
fn a_shortcut_with_modifiers_runs_its_command() {
    let mut h = window();
    ok(&mut h, "ui.key", json!({"key":"O","modifiers":["ctrl"]}));
    assert_eq!(h.state().fixture.ran, ["file.open"]);
    ok(
        &mut h,
        "ui.key",
        json!({"key":"S","modifiers":["ctrl","shift"]}),
    );
    assert_eq!(h.state().fixture.ran, ["file.open", "file.save-as"]);
}

#[test]
fn pointer_press_and_release_follow_the_menu_rules() {
    let mut h = window();
    let tree = ok(&mut h, "ui.tree", json!({"label":"Edit"}));
    let r = &node(&tree, "Edit")["rect"];
    let (x, y) = (
        (r[0].as_f64().unwrap() + r[2].as_f64().unwrap()) / 2.0,
        (r[1].as_f64().unwrap() + r[3].as_f64().unwrap()) / 2.0,
    );
    let pressed = ok(&mut h, "ui.pointer", json!({"x":x,"y":y,"action":"press"}));
    assert_eq!(pressed["menu"]["menu"], "Edit", "open on press");
    ok(
        &mut h,
        "ui.pointer",
        json!({"x":x,"y":y,"action":"release"}),
    );
    assert!(
        menu::current(&h.ctx).is_some(),
        "the opening release keeps it open"
    );
    ok(&mut h, "ui.pointer", json!({"x":x,"y":y,"action":"press"}));
    let closed = ok(
        &mut h,
        "ui.pointer",
        json!({"x":x,"y":y,"action":"release"}),
    );
    assert_eq!(closed["menu"], Value::Null);
}

#[test]
fn typing_reaches_the_focused_text_field() {
    let mut h = window();
    let clicked = ok(&mut h, "ui.click", json!({"label":"Name"}));
    assert_eq!(
        clicked["focused"]["placeholder"], "Name",
        "a click by placeholder focuses the field"
    );
    let typed = ok(&mut h, "ui.type", json!({"text":"Ada"}));
    assert_eq!(h.state().name, "Ada");
    assert_eq!(typed["focused"]["value"], "Ada");
}

#[test]
fn scrolling_moves_the_list_under_the_pointer() {
    let mut h = window();
    let before = ok(&mut h, "ui.tree", json!({"label":"Row 0"}));
    let r = node(&before, "Row 0")["rect"].clone();
    let top = r[1].as_f64().unwrap();
    ok(
        &mut h,
        "ui.scroll",
        json!({"x":r[0].as_f64().unwrap() + 4.0,"y":top + 4.0,"dx":0,"dy":-200}),
    );
    h.run_steps(30);
    let after = ok(&mut h, "ui.tree", json!({"label":"Row 0"}));
    let moved = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["label"] == "Row 0")
        .map(|n| n["rect"][1].as_f64().unwrap());
    assert!(
        moved.is_none_or(|y| y < top),
        "Row 0 scrolled up or out: {moved:?} from {top}"
    );
}

#[test]
fn unknown_and_ambiguous_targets_are_refused() {
    let mut h = window();
    let missing = call(&mut h, "ui.click", json!({"label":"Nope"}));
    assert_eq!(
        (missing.rc, missing.body["error_code"].as_str()),
        (10, Some("ARGUMENT"))
    );
    let unknown = call(&mut h, "ui.wave", json!({}));
    assert_eq!(unknown.body["error_code"], "UNKNOWN_VERB");
}

#[test]
fn queued_inputs_run_in_order() {
    let mut h = window();
    let app = h.state_mut();
    app.inbox.push((101, "ui.click", json!({"label":"File"})));
    app.inbox.push((102, "ui.key", json!({"key":"Escape"})));
    app.next = 200;
    for _ in 0..30 {
        h.step();
    }
    let answers = &h.state().answers;
    assert_eq!(answers.iter().map(|a| a.id).collect::<Vec<_>>(), [101, 102]);
    assert_eq!(answers[0].body["menu"]["menu"], "File");
    assert_eq!(answers[1].body["menu"], Value::Null);
}

#[test]
fn window_verbs_send_viewport_commands() {
    let mut h = window();
    let maximized = ok(&mut h, "window", json!({"action":"maximize"}));
    assert_eq!(maximized["requested"], "maximize");
    assert!(maximized["pixels_per_point"].is_number());
    ok(&mut h, "window", json!({"action":"restore"}));
    ok(&mut h, "window", json!({"action":"focus"}));
    let closing = ok(&mut h, "window", json!({"action":"close"}));
    assert_eq!(closing["requested"], "close");
    let sent = sent(&h);
    for expected in [
        ViewportCommand::Maximized(true),
        ViewportCommand::Minimized(false),
        ViewportCommand::Maximized(false),
        ViewportCommand::Focus,
        ViewportCommand::Close,
    ] {
        assert!(sent.contains(&expected), "{expected:?} in {sent:?}");
    }
    let state = ok(&mut h, "window.state", json!({}));
    assert!(state["size"].is_array());
}

/// The harness paints and delivers screenshots as the native backend does.
#[test]
fn a_capture_writes_the_window_to_a_png() {
    use std::os::unix::fs::PermissionsExt;
    let mut h = window();
    let dir = captures(&h);
    let shot = ok(&mut h, "ui.capture", json!({"name":"window.png"}));
    assert!(
        sent(&h)
            .iter()
            .any(|c| matches!(c, ViewportCommand::Screenshot(_)))
    );
    let path = dir.join("window.png");
    assert_eq!(shot["path"], json!(path));
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(900), Some(500))
    );
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700,
        "the capture directory is private"
    );
    let generated = ok(&mut h, "ui.capture", json!({}));
    assert!(
        generated["path"]
            .as_str()
            .unwrap()
            .starts_with(dir.to_str().unwrap()),
        "{generated}"
    );
}

/// `<runtime>/demo/captures`, where every capture of the test window goes.
fn captures(h: &Harness<'_, App>) -> std::path::PathBuf {
    h.state()
        .runtime
        .as_ref()
        .unwrap()
        .path()
        .join("demo/captures")
}

fn refused(h: &mut Harness<'_, App>, verb: &'static str, args: Value) -> String {
    let answer = call(h, verb, args);
    assert_eq!(answer.rc, 10, "{verb} must be refused: {}", answer.body);
    answer.body["error_code"].as_str().unwrap().to_owned()
}

/// sol finding 1: a capture never writes outside its directory, over an
/// existing file or through a link.
#[test]
fn a_capture_refuses_paths_existing_files_and_links() {
    let mut h = window();
    ok(&mut h, "ui.capture", json!({"name":"first.png"}));
    let dir = captures(&h);
    let outside = h
        .state()
        .runtime
        .as_ref()
        .unwrap()
        .path()
        .join("precious.txt");
    std::fs::write(&outside, "keep me").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("link.png")).unwrap();
    for name in [
        "first.png",
        "link.png",
        "../escape.png",
        "a/b.png",
        "..png",
        ".hidden.png",
        "shot.jpg",
        "",
    ] {
        assert_eq!(
            refused(&mut h, "ui.capture", json!({"name":name})),
            "ARGUMENT",
            "{name:?}"
        );
    }
    assert_eq!(
        refused(&mut h, "ui.capture", json!({"path":outside})),
        "ARGUMENT",
        "no caller paths"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
    assert!(!dir.join("../escape.png").exists());
}

/// A link planted after the request but before the screenshot arrives is
/// still refused: the file is created exclusively.
#[test]
fn a_capture_refuses_a_link_planted_while_it_waits() {
    let mut h = window();
    ok(&mut h, "ui.capture", json!({"name":"first.png"}));
    let outside = h
        .state()
        .runtime
        .as_ref()
        .unwrap()
        .path()
        .join("precious.txt");
    std::fs::write(&outside, "keep me").unwrap();
    h.state_mut()
        .inbox
        .push((50, "ui.capture", json!({"name":"late.png"})));
    h.step();
    std::os::unix::fs::symlink(&outside, captures(&h).join("late.png")).unwrap();
    for _ in 0..20 {
        h.step();
    }
    let answer = h
        .state()
        .answers
        .iter()
        .find(|a| a.id == 50)
        .cloned()
        .expect("answered");
    assert_eq!(
        (answer.rc, answer.body["error_code"].as_str()),
        (10, Some("CAPTURE")),
        "{}",
        answer.body
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
}

/// live finding A: node ids are decimal text, which keeps all 64 bits, and
/// a click takes them back as text or as an exact integer.
#[test]
fn node_ids_round_trip_as_text() {
    let mut h = window();
    let tree = ok(&mut h, "ui.tree", json!({}));
    assert!(tree["root"].is_string() && tree["focus"].is_string());
    let file = node(&tree, "File").clone();
    let id = file["id"].as_str().expect("a text id").to_owned();
    assert!(
        file["children"]
            .as_array()
            .unwrap()
            .iter()
            .all(Value::is_string)
    );
    let wide = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str()?.parse::<u64>().ok())
        .any(|v| v > 1 << 53);
    assert!(wide, "egui ids use the full 64 bits, so text is needed");
    let opened = ok(&mut h, "ui.click", json!({"id":id}));
    assert_eq!(opened["menu"]["menu"], "File");
    assert_eq!(opened["target"]["id"], json!(id));
    ok(&mut h, "ui.key", json!({"key":"Escape"}));
    let exact: u64 = id.parse().unwrap();
    assert_eq!(
        ok(&mut h, "ui.click", json!({"id":exact}))["target"]["id"],
        json!(id)
    );
    assert_eq!(
        refused(&mut h, "ui.click", json!({"id":"1.5e19"})),
        "ARGUMENT"
    );
}

/// sol finding 4: hiding the window answers what it accepted, instead of
/// stranding input no pass will ever read.
#[test]
fn hiding_the_window_settles_accepted_input() {
    let mut h = window();
    let app = h.state_mut();
    app.inbox.push((60, "ui.click", json!({"label":"File"})));
    app.inbox.push((61, "ui.type", json!({"text":"x"})));
    // Accepted; then hidden before its press and release pass is read.
    h.step();
    h.input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .minimized = Some(true);
    h.step();
    let answers = &h.state().answers;
    assert_eq!(
        answers.iter().map(|a| a.id).collect::<Vec<_>>(),
        [60, 61],
        "{answers:?}"
    );
    assert!(
        answers
            .iter()
            .all(|a| a.rc == 10 && a.body["error_code"] == "CANCELLED"),
        "{answers:?}"
    );
    assert!(!drive_pending(&h));
    assert_eq!(
        refused(&mut h, "ui.key", json!({"key":"A"})),
        "BUSY",
        "hidden: no new input"
    );
    h.input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .minimized = Some(false);
    ok(&mut h, "ui.click", json!({"label":"File"}));
}

/// Minimizing over the Bus settles queued work before the window stops.
#[test]
fn minimizing_settles_queued_work_first() {
    let mut h = window();
    let app = h.state_mut();
    app.inbox.push((70, "ui.click", json!({"label":"File"})));
    app.inbox.push((71, "ui.key", json!({"key":"Escape"})));
    app.inbox.push((72, "window", json!({"action":"minimize"})));
    h.step();
    let ids: Vec<_> = h
        .state()
        .answers
        .iter()
        .map(|a| (a.id, a.body["error_code"].as_str().map(str::to_owned)))
        .collect();
    assert_eq!(
        ids,
        [
            (70, Some("CANCELLED".into())),
            (71, Some("CANCELLED".into())),
            (72, None)
        ]
    );
    assert!(sent(&h).contains(&ViewportCommand::Minimized(true)));
}

/// sol re-review 2: a click cancelled before its press and release (the
/// window hidden, so eframe runs logic only and no pass) leaves no button
/// down, and on restore the click never fires. A click's press and release
/// share one pass, so no click is ever cut between them.
#[test]
fn a_click_cancelled_before_its_press_leaves_no_button_down() {
    let mut h = window();
    h.state_mut()
        .inbox
        .push((90, "ui.click", json!({"label":"Count"})));
    h.step(); // accepted
    h.step(); // moved
    assert!(!h.ctx.input(|i| i.pointer.any_down()), "not pressed yet");
    let mut hidden = h.input_mut().clone();
    hidden
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .minimized = Some(true);
    let mut answers = Vec::new();
    let _ = h
        .ctx
        .run_logic(&hidden, |ctx| answers.extend(drive::logic(ctx)));
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(
        (answers[0].id, answers[0].body["error_code"].as_str()),
        (90, Some("CANCELLED"))
    );
    for _ in 0..5 {
        h.step();
    }
    assert!(
        !h.ctx.input(|i| i.pointer.any_down()),
        "no button is left down"
    );
    assert_eq!(h.state().count, 0, "the cancelled click never fired");
    ok(&mut h, "ui.click", json!({"label":"Count"}));
    assert_eq!(h.state().count, 1, "a later click still works");
}

/// sol finding 3, toolkit half: closing answers every accepted request.
#[test]
fn finishing_answers_running_and_queued_work() {
    let mut h = window();
    let app = h.state_mut();
    app.inbox.push((80, "ui.click", json!({"label":"File"})));
    app.inbox.push((81, "ui.key", json!({"key":"Escape"})));
    app.inbox.push((82, "ui.capture", json!({})));
    // Accepted; closing comes before its press and release are read.
    h.step();
    h.state_mut().finish = true;
    h.step();
    let answers = &h.state().answers;
    assert_eq!(
        answers.iter().map(|a| a.id).collect::<Vec<_>>(),
        [80, 81, 82]
    );
    assert_eq!(
        (
            answers[0].rc,
            &answers[0].body["closing"],
            &answers[0].body["interrupted"]
        ),
        (0, &json!(true), &json!(true))
    );
    assert!(
        answers[1..]
            .iter()
            .all(|a| a.body["error_code"] == "CANCELLED")
    );
    assert!(!drive_pending(&h));
}

fn drive_pending(h: &Harness<'_, App>) -> bool {
    drive::pending(&h.ctx)
}

/// Only the screenshot this capture asked for is written.
#[test]
fn a_capture_ignores_other_screenshots() {
    let mut h = window();
    h.state_mut()
        .inbox
        .push((7, "ui.capture", json!({"name":"mine.png"})));
    h.step();
    h.step();
    // A screenshot someone else asked for arrives while this one waits.
    h.event(Event::Screenshot {
        viewport_id: ViewportId::ROOT,
        user_data: egui::UserData::new(7_u64),
        image: Arc::new(ColorImage::filled([4, 3], Color32::RED)),
    });
    for _ in 0..20 {
        h.step();
    }
    let shot = h
        .state()
        .answers
        .iter()
        .find(|a| a.id == 7)
        .cloned()
        .expect("answered");
    assert_eq!(
        shot.body["width"], 900,
        "not the stray 4×3 image: {}",
        shot.body
    );
}
