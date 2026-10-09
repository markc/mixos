// SPDX-License-Identifier: MIT OR Apache-2.0
//! BusViewer driven over its Bus verbs the way the running shell is: every
//! command enters the engine, is settled, and window-level verbs go to the
//! toolkit drive layer, against the real view. What an agent clicks and
//! types lands in the engine as a person's input would.
use busviewer::shell::{apply_ui, settle};
use busviewer::view::{UiEvent, view};
use busviewer::{commands, label, strings};
use egui_kittest::Harness;
use inspector::model::Verb;
use inspector::{Effect, Engine, Snapshot};
use serde_json::{Value, json};
use toolkit::drive::{self, Answer};
use toolkit::{Theme, icons};

struct Window {
    engine: Engine,
    inbox: Vec<(u64, &'static str, Value)>,
    answers: Vec<Answer>,
}

fn engine() -> Engine {
    let verb = |name: &str| Verb { name: name.into(), args: String::new(), description: String::new(), read_only: Some(true) };
    let mut s = Snapshot::default();
    s.services.insert("settingsd".into(), Ok(vec![verb("settings.get"), verb("settings.list")]));
    s.services.insert("noded".into(), Ok(vec![verb("noded.list")]));
    let mut e = Engine::new(label);
    let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!("initial discovery") };
    e.discovered(ticket, s);
    e.take_effects();
    e
}

fn window() -> Harness<'static, Window> {
    let registry = commands::registry();
    let strings = strings();
    let theme = Theme::embedded();
    let stroke = icons::stroke_width(&theme);
    let state = Window { engine: engine(), inbox: Vec::new(), answers: Vec::new() };
    let mut harness = Harness::builder().with_size(egui::vec2(980.0, 620.0)).build_ui_state(
        move |ui, w: &mut Window| {
            for (id, verb, args) in std::mem::take(&mut w.inbox) {
                w.engine.command(id, verb, &args.to_string());
            }
            let mut effects = settle(&mut w.engine, &registry, &strings);
            let fired = registry.shortcuts(ui.ctx(), &w.engine);
            let mut events = view(ui, &w.engine, &registry, &strings, stroke);
            events.extend(fired.into_iter().map(UiEvent::Command));
            apply_ui(&mut w.engine, &registry, events);
            effects.extend(settle(&mut w.engine, &registry, &strings));
            for effect in effects {
                match effect {
                    Effect::Reply { id, rc, body } => w.answers.push(Answer { id, rc, body }),
                    Effect::Drive { id, verb, args } => w.answers.extend(drive::request(ui.ctx(), id, &verb, &args)),
                    _ => {}
                }
            }
            w.answers.extend(drive::logic(ui.ctx()));
        },
        state,
    );
    toolkit::install(&harness.ctx, &theme);
    drive::install(&harness.ctx, busviewer::shell::APP);
    harness.run_steps(2);
    harness
}

fn ok(h: &mut Harness<'_, Window>, id: u64, verb: &'static str, args: Value) -> Value {
    h.state_mut().inbox.push((id, verb, args));
    for _ in 0..60 {
        h.step();
        if let Some(at) = h.state().answers.iter().position(|a| a.id == id) {
            let answer = h.state_mut().answers.remove(at);
            assert_eq!(answer.rc, 0, "{verb}: {}", answer.body);
            return answer.body;
        }
    }
    panic!("{verb} was never answered");
}

#[test]
fn clicking_and_typing_in_the_search_field_filters_the_tree() {
    let mut h = window();
    let clicked = ok(&mut h, 1, "busviewer.ui.click", json!({"label":label("search")}));
    assert_eq!(clicked["focused"]["placeholder"], label("search"));
    ok(&mut h, 2, "busviewer.ui.type", json!({"text":"list"}));
    assert_eq!(h.state().engine.ui.filter, "list");
    let rows = ok(&mut h, 3, "busviewer.tree", json!({}));
    let keys: Vec<_> = rows["rows"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap().to_owned()).collect();
    assert!(keys.contains(&"verb:settingsd:settings.list".to_owned()), "{keys:?}");
    assert!(!keys.contains(&"verb:settingsd:settings.get".to_owned()), "{keys:?}");
}

#[test]
fn a_bus_edit_shows_in_the_window_tree() {
    let mut h = window();
    ok(&mut h, 1, "busviewer.expand", json!({"key":"service:settingsd","open":true}));
    h.run_steps(2);
    let tree = ok(&mut h, 2, "busviewer.ui.tree", json!({"label":"settings.get"}));
    let node = tree["nodes"].as_array().unwrap().first().cloned().expect("the verb row is drawn");
    let id = node["id"].as_str().expect("a text node id").to_owned();
    ok(&mut h, 3, "busviewer.ui.click", json!({"id":id}));
    let selected = h.state().engine.ui.selected.clone().map(|s| s.verb);
    assert_eq!(selected.as_deref(), Some("settings.get"), "the click selected the verb");
    assert_eq!(h.state().engine.ui.row_key.as_deref(), Some("verb:settingsd:settings.get"));
}

/// The nodes `busviewer.ui.tree` lists whose label contains `label`.
fn nodes(h: &mut Harness<'_, Window>, id: u64, label: &str) -> Vec<Value> {
    ok(h, id, "busviewer.ui.tree", json!({"label":label}))["nodes"].as_array().cloned().unwrap_or_default()
}

#[test]
fn the_title_bar_controls_are_in_the_tree_and_work_by_click() {
    let mut h = window();
    for name in [label("search-services"), label("toggle-mode"), label("shortcuts")] {
        let found = nodes(&mut h, 1, &name);
        assert!(found.iter().any(|n| matches!(n["role"].as_str(), Some("Button" | "ToggleButton" | "Link"))), "{name}: {found:?}");
        assert!(found.iter().all(|n| n["enabled"] == true), "{name} is enabled");
    }
    let search = nodes(&mut h, 2, &label("search-services"));
    assert!(search[0]["label"].as_str().is_some_and(|l| l.contains('(')), "its tooltip names the shortcut: {search:?}");

    ok(&mut h, 3, "busviewer.ui.click", json!({"label":search[0]["label"]}));
    h.run_steps(2);
    assert!(!h.state().engine.ui.focus_filter, "the view took the focus request");
    ok(&mut h, 4, "busviewer.ui.click", json!({"label":label("toggle-mode")}));
    assert!(h.state().engine.ui.invert_mode, "the toggle flips the window's mode");
    ok(&mut h, 5, "busviewer.ui.click", json!({"label":label("shortcuts")}));
    assert_eq!(h.state().engine.ui.dialog, Some(inspector::Dialog::Shortcuts), "the link opens the shortcuts");
}

#[test]
fn search_from_the_title_bar_puts_the_keyboard_in_the_services_filter() {
    let mut h = window();
    let search = nodes(&mut h, 1, &label("search-services"));
    let answer = ok(&mut h, 2, "busviewer.ui.click", json!({"label":search[0]["label"]}));
    h.run_steps(2);
    let focused = ok(&mut h, 3, "busviewer.ui.key", json!({"key":"End"}));
    assert_eq!(focused["focused"]["placeholder"], label("search"), "{answer} / {focused}");
    ok(&mut h, 4, "busviewer.ui.type", json!({"text":"noded"}));
    assert_eq!(h.state().engine.ui.filter, "noded");
}

#[test]
fn every_chrome_control_of_the_window_is_in_the_tree_with_a_label() {
    let mut h = window();
    // The tree comes depth first as one list.
    let all = ok(&mut h, 1, "busviewer.ui.tree", json!({}))["nodes"].as_array().cloned().unwrap();
    let labelled = |name: &str| all.iter().any(|n| n["label"].as_str().is_some_and(|l| l.contains(name)));
    let wanted = [
        label("file"), label("edit"), label("bus"), label("help"),
        "Minimize".to_owned(), "Maximize".to_owned(), "Close".to_owned(),
        label("search-services"), label("toggle-mode"), label("shortcuts"),
        label("services"), label("details"), "Panel menu".to_owned(),
        label("call"), label("clear"),
    ];
    for name in &wanted {
        assert!(labelled(name), "{name} is missing or unlabelled");
    }
    assert_eq!(all.iter().filter(|n| n["label"] == "Panel menu").count(), 2, "one per panel group");
    assert!(all.iter().any(|n| n["placeholder"] == label("search")), "the search field names itself by its hint");
    // Every button-like control has a label.
    for node in &all {
        if matches!(node["role"].as_str(), Some("Button" | "ToggleButton" | "Link" | "CheckBox" | "ComboBox" | "Slider" | "SpinButton")) {
            assert!(node["label"].as_str().is_some_and(|l| !l.is_empty()), "unlabelled {node}");
        }
    }
}

#[test]
fn the_menu_bar_opens_and_runs_commands_over_the_bus() {
    let mut h = window();
    let opened = ok(&mut h, 1, "busviewer.ui.click", json!({"label":label("help")}));
    assert_eq!(opened["menu"]["menu"], label("help"));
    let menu = ok(&mut h, 2, "busviewer.ui.menu", json!({}));
    assert_eq!(menu["menu"]["menu"], label("help"));
    ok(&mut h, 3, "busviewer.ui.click", json!({"label":label("about")}));
    assert_eq!(h.state().engine.ui.dialog, Some(inspector::Dialog::About));
    ok(&mut h, 4, "busviewer.ui.key", json!({"key":"Escape"}));
    assert_eq!(h.state().engine.ui.dialog, None, "Escape closes the dialog");
    let state = ok(&mut h, 5, "busviewer.window.state", json!({}));
    assert!(state["pixels_per_point"].is_number());
}
