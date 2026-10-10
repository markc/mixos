// SPDX-License-Identifier: MIT OR Apache-2.0
//! The real [`Shell`] on a logging Bus handle: Bus commands arrive through
//! its delivery channel and its effect runtime, exactly as from noded, and
//! every reply, call and the final quit is read back from the handle in the
//! order the shell made them.
use busviewer::label;
use busviewer::shell::{APP, Shell};
use egui_kittest::Harness;
use futures::channel::mpsc;
use inspector::bus::{Delivery, Handle, Logged};
use inspector::model::Verb;
use inspector::{Engine, Snapshot};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use toolkit::{Theme, drive};

struct Rig {
    h: Harness<'static, Option<Shell>>,
    bus: Handle,
    deliveries: mpsc::Sender<Delivery>,
}

fn fixture() -> Snapshot {
    let verb = |name: &str| Verb {
        name: name.into(),
        args: String::new(),
        description: String::new(),
        read_only: Some(true),
    };
    let mut s = Snapshot::default();
    s.services.insert("example".into(), Ok(vec![verb("echo")]));
    s
}

fn engine(rig: &Rig) -> &Engine {
    rig.h.state().as_ref().expect("a shell").engine()
}

fn rig() -> Rig {
    rig_with(Theme::embedded(), Harness::builder())
}

/// The rig as the live window runs: a chrome scheme, and passes a 60th of
/// a second apart (the harness default is a quarter second).
fn live_rig() -> Rig {
    let theme = Theme::for_context(design::DesignContext {
        scheme: design::Scheme::Pro,
        mode: design::Mode::Light,
        ..Default::default()
    });
    rig_with(
        theme,
        Harness::builder()
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(240),
    )
}

fn rig_with(theme: Theme, builder: egui_kittest::HarnessBuilder<Option<Shell>>) -> Rig {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let mut h = builder.with_size(egui::vec2(980.0, 620.0)).build_ui_state(
        |ui, shell: &mut Option<Shell>| {
            if let Some(shell) = shell {
                shell.logic(ui.ctx());
                shell.ui(ui);
            }
        },
        None,
    );
    toolkit::install(&h.ctx, &theme);
    let runtime = std::env::temp_dir().join(format!(
        "busviewer-shell-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&runtime).unwrap();
    // Installed first, so the shell's own install keeps these options.
    drive::install_with(
        &h.ctx,
        APP,
        drive::Options {
            runtime_dir: Some(runtime),
            ..drive::Options::default()
        },
    );
    let bus = Handle::sink();
    let (deliveries, rx) = mpsc::channel(64);
    let shell = Shell::new(h.ctx.clone(), theme, bus.clone(), rx, "comp".into()).unwrap();
    *h.state_mut() = Some(shell);
    let mut rig = Rig { h, bus, deliveries };
    // The first discovery fails unsent on the sink; then load a node.
    step_until(&mut rig, "the first discovery", |rig| !engine(rig).busy());
    rig.h.state_mut().as_mut().unwrap().engine_mut().snapshot = fixture();
    rig.h.run_steps(3);
    rig
}

fn step_until(rig: &mut Rig, what: &str, done: impl Fn(&Rig) -> bool) {
    for _ in 0..400 {
        rig.h.step();
        if done(rig) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{what} never happened: {:?}", rig.bus.log());
}

fn send(rig: &mut Rig, id: u64, verb: &str, args: Value) {
    rig.deliveries
        .try_send(Delivery::Command {
            id,
            verb: verb.into(),
            body: args.to_string(),
        })
        .unwrap();
}

fn reply(rig: &Rig, id: u64) -> Option<(usize, u8, Value)> {
    rig.bus
        .log()
        .into_iter()
        .enumerate()
        .find_map(|(at, l)| match l {
            Logged::Reply { id: i, rc, body } if i == id => Some((at, rc, body)),
            _ => None,
        })
}

fn wait(rig: &mut Rig, ids: &[u64]) {
    step_until(rig, &format!("replies {ids:?}"), |rig| {
        ids.iter().all(|id| reply(rig, *id).is_some())
    });
}

fn quit_at(rig: &Rig) -> Option<usize> {
    rig.bus.log().iter().position(|l| *l == Logged::Quit)
}

/// The body editor's node id (its placeholder is also a heading's text, so
/// it is clicked by id).
fn body_editor(rig: &mut Rig) -> String {
    send(rig, 900, "busviewer.ui.tree", json!({}));
    wait(rig, &[900]);
    let (_, _, tree) = reply(rig, 900).unwrap();
    let editor = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["placeholder"] == label("body") && n["role"] != "Label");
    editor.expect("the body editor")["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// sol finding 2: an engine command waits behind drive input accepted
/// before it, so the call sends the body that was typed.
#[test]
fn a_call_after_typed_input_sends_the_typed_body() {
    let mut rig = rig();
    send(
        &mut rig,
        1,
        "busviewer.select",
        json!({"service":"example","verb":"echo"}),
    );
    wait(&mut rig, &[1]);
    let editor = body_editor(&mut rig);
    send(&mut rig, 2, "busviewer.ui.click", json!({"id":editor}));
    send(
        &mut rig,
        3,
        "busviewer.ui.type",
        json!({"text":"{\"n\":2}"}),
    );
    send(&mut rig, 4, "busviewer.call", json!({}));
    send(&mut rig, 5, "busviewer.info", json!({}));
    wait(&mut rig, &[2, 3, 4, 5]);
    let calls: Vec<_> = rig
        .bus
        .log()
        .into_iter()
        .filter(|l| matches!(l, Logged::Call { service, .. } if service == "example"))
        .collect();
    assert_eq!(
        calls,
        [Logged::Call {
            service: "example".into(),
            verb: "echo".into(),
            body: "{\"n\":2}".into()
        }]
    );
    // The call answers when it completes, so only the input is strictly
    // before everything after it.
    let at = |id| reply(&rig, id).unwrap().0;
    assert!(
        at(2) < at(3) && at(3) < at(4) && at(3) < at(5),
        "{:?}",
        rig.bus.log()
    );
    assert_eq!(reply(&rig, 4).unwrap().2["request_body"], "{\"n\":2}");
    assert_eq!(
        reply(&rig, 5).unwrap().2["body"],
        "{\"n\":2}",
        "info read after the typing"
    );
}

/// sol finding 3: Ctrl+Q injected over the Bus answers its own request,
/// cancels the queued input and capture, refuses the held command, and only
/// then stops the Bus.
#[test]
fn an_injected_quit_answers_everything_before_the_bus_stops() {
    let mut rig = rig();
    send(
        &mut rig,
        1,
        "busviewer.ui.key",
        json!({"key":"Q","modifiers":["ctrl"]}),
    );
    send(&mut rig, 2, "busviewer.ui.click", json!({"label":"File"}));
    send(&mut rig, 3, "busviewer.ui.capture", json!({}));
    send(&mut rig, 4, "busviewer.info", json!({}));
    step_until(&mut rig, "the quit", |rig| quit_at(rig).is_some());
    let quit = quit_at(&rig).unwrap();
    for id in 1..=4 {
        let (at, _, _) =
            reply(&rig, id).unwrap_or_else(|| panic!("{id} unanswered: {:?}", rig.bus.log()));
        assert!(at < quit, "{id} answered after the Bus stopped");
    }
    let (_, rc, body) = reply(&rig, 1).unwrap();
    assert_eq!((rc, &body["closing"]), (0, &json!(true)), "{body}");
    for id in [2, 3] {
        assert_eq!(reply(&rig, id).unwrap().2["error_code"], "CANCELLED");
    }
    assert_eq!(reply(&rig, 4).unwrap().2["error_code"], "BUSY");
    assert!(engine(&rig).quitting);
}

/// sol re-review 1: an idle quit with commands behind it, some already
/// forwarded and some still in the delivery inbox, answers every one of
/// them before the Bus stops.
#[test]
fn a_quit_answers_every_command_behind_it_before_the_bus_stops() {
    let mut rig = rig();
    send(&mut rig, 1, "busviewer.quit", json!({}));
    for id in 2..=6 {
        send(&mut rig, id, "busviewer.info", json!({}));
    }
    // Let the forwarder move those into the shell's queue, unpumped.
    std::thread::sleep(Duration::from_millis(50));
    for id in 7..=12 {
        send(&mut rig, id, "busviewer.ping", json!({}));
    }
    step_until(&mut rig, "the quit", |rig| quit_at(rig).is_some());
    let quit = quit_at(&rig).unwrap();
    for id in 1..=12 {
        let (at, rc, body) =
            reply(&rig, id).unwrap_or_else(|| panic!("{id} unanswered: {:?}", rig.bus.log()));
        assert!(at < quit, "{id} answered after the Bus stopped");
        if id == 1 {
            assert_eq!((rc, &body["quitting"]), (0, &json!(true)));
        } else {
            assert_eq!(
                (rc, body["error_code"].as_str()),
                (10, Some("BUSY")),
                "{id}: {body}"
            );
        }
    }
    // Nothing more is admitted once closing.
    assert!(
        rig.deliveries
            .try_send(Delivery::Command {
                id: 13,
                verb: "busviewer.ping".into(),
                body: "{}".into()
            })
            .is_err()
    );
}

/// sol finding 3: busviewer.quit waits for the drive input before it.
#[test]
fn quit_waits_for_drive_input_accepted_before_it() {
    let mut rig = rig();
    send(
        &mut rig,
        1,
        "busviewer.ui.click",
        json!({"label":label("help")}),
    );
    send(&mut rig, 2, "busviewer.ui.key", json!({"key":"Escape"}));
    send(&mut rig, 3, "busviewer.quit", json!({}));
    step_until(&mut rig, "the quit", |rig| quit_at(rig).is_some());
    let quit = quit_at(&rig).unwrap();
    let (click, rc, body) = reply(&rig, 1).unwrap();
    assert_eq!(
        (rc, &body["menu"]["menu"]),
        (0, &json!(label("help"))),
        "{body}"
    );
    let (key, rc, _) = reply(&rig, 2).unwrap();
    assert_eq!(rc, 0);
    let (quitting, rc, _) = reply(&rig, 3).unwrap();
    assert_eq!(rc, 0);
    assert!(
        click < key && key < quitting && quitting < quit,
        "{:?}",
        rig.bus.log()
    );
}

/// sol finding 5: a split set over the Bus is the width the window draws,
/// and stays the engine's split afterwards.
#[test]
fn a_bus_split_is_the_rendered_split() {
    let mut rig = rig();
    let panel = egui::Id::new("services");
    let width = |rig: &Rig| {
        egui::containers::panel::PanelState::load(&rig.h.ctx, panel).map(|s| s.size().x)
    };
    let before = width(&rig).expect("the services panel");
    assert!((before / 980.0 - 0.34).abs() < 0.02, "{before}");
    send(&mut rig, 1, "busviewer.split", json!({"value":0.5}));
    wait(&mut rig, &[1]);
    assert_eq!(reply(&rig, 1).unwrap().2["split"], 0.5);
    rig.h.run_steps(5);
    let after = width(&rig).unwrap();
    assert!(
        (after / 980.0 - 0.5).abs() < 0.01,
        "rendered {after} of 980"
    );
    assert!(
        (engine(&rig).ui.split - 0.5).abs() < 0.01,
        "the view kept it: {}",
        engine(&rig).ui.split
    );
}

/// Ask `busviewer.ui.tree` with `args` and answer its nodes.
fn tree(rig: &mut Rig, id: u64, args: Value) -> Vec<Value> {
    send(rig, id, "busviewer.ui.tree", args);
    wait(rig, &[id]);
    reply(rig, id).unwrap().2["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The one control named `name` exactly.
fn control(rig: &mut Rig, id: u64, name: &str) -> String {
    let nodes = tree(rig, id, json!({"label":name,"exact":true,"role":"Button"}));
    assert_eq!(nodes.len(), 1, "{name}: {nodes:?}");
    nodes[0]["id"].as_str().unwrap().to_owned()
}

/// Live bug 1: a Bus click on the light/dark toggle (move, press in one
/// pass, release in the next, as the drive layer injects it) flips the
/// window's mode exactly once per click, though the click reinstalls the
/// theme.
#[test]
fn a_bus_click_on_the_mode_toggle_flips_it_exactly_once() {
    let mut rig = live_rig();
    let toggle = control(&mut rig, 1, &label("toggle-mode"));
    use design::Mode::{Dark, Light};
    for (n, expected) in [(10, Some(Dark)), (20, Some(Light)), (30, Some(Dark))] {
        let before = engine(&rig).ui.theme_mode;
        send(&mut rig, n, "busviewer.ui.click", json!({"id":toggle}));
        wait(&mut rig, &[n]);
        assert_eq!(reply(&rig, n).unwrap().1, 0);
        // Idle passes after the click, as a live window keeps drawing.
        rig.h.run_steps(30);
        assert_eq!(
            engine(&rig).ui.theme_mode,
            expected,
            "click {n} from {before:?}"
        );
    }
    // And the window shows it: Pro light's opposite, Pro dark, is installed.
    assert_eq!(
        rig.h.ctx.global_style().visuals.panel_fill,
        egui::Color32::from_rgb(0x32, 0x32, 0x32)
    );
}

/// Live bug 1, its cause: the VT4 window, unfocused, draws once a second.
/// A press and release a pass apart were a one-second press, not a click,
/// so every title-bar control ignored ui.click. At one pass a second each
/// control now works from a Bus click.
#[test]
fn title_bar_controls_click_at_one_pass_a_second() {
    let theme = Theme::for_context(design::DesignContext {
        scheme: design::Scheme::Pro,
        mode: design::Mode::Light,
        ..Default::default()
    });
    let mut rig = rig_with(theme, Harness::builder().with_step_dt(1.0));
    let toggle = control(&mut rig, 1, &label("toggle-mode"));
    send(&mut rig, 2, "busviewer.ui.click", json!({"id":toggle}));
    wait(&mut rig, &[2]);
    rig.h.run_steps(2);
    assert_eq!(
        engine(&rig).ui.theme_mode,
        Some(design::Mode::Dark),
        "the toggle flipped"
    );
    let search = tree(
        &mut rig,
        3,
        json!({"label":label("search-services"),"role":"Button"}),
    );
    let search = search[0]["id"].as_str().unwrap().to_owned();
    send(&mut rig, 4, "busviewer.ui.click", json!({"id":search}));
    wait(&mut rig, &[4]);
    rig.h.run_steps(2);
    let focused = tree(&mut rig, 5, json!({}));
    let focus = focused
        .iter()
        .find(|n| n["focused"] == true)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        focus["placeholder"],
        label("search"),
        "the filter has the keyboard: {focus}"
    );
    let link = control_link(&mut rig, 6, &label("shortcuts"));
    send(&mut rig, 7, "busviewer.ui.click", json!({"id":link}));
    wait(&mut rig, &[7]);
    rig.h.run_steps(2);
    assert_eq!(
        engine(&rig).ui.dialog,
        Some(inspector::Dialog::Shortcuts),
        "the link opened the shortcuts"
    );
}

/// A dialog opened over the Bus is a modal Dialog node to AccessKit, named
/// by its title with its Done button inside; closed over the Bus, it is
/// gone and the title bar takes clicks again.
#[test]
fn a_bus_dialog_is_an_accessible_modal_and_a_bus_close_frees_the_window() {
    let theme = Theme::for_context(design::DesignContext {
        scheme: design::Scheme::Pro,
        mode: design::Mode::Light,
        ..Default::default()
    });
    let mut rig = rig_with(theme, Harness::builder().with_step_dt(1.0));
    send(&mut rig, 1, "busviewer.dialog", json!({"open":"shortcuts"}));
    wait(&mut rig, &[1]);
    assert_eq!(reply(&rig, 1).unwrap().2["dialog"], "shortcuts");
    rig.h.run_steps(3);
    let all = tree(&mut rig, 2, json!({}));
    let dialogs: Vec<_> = all.iter().filter(|n| n["role"] == "Dialog").collect();
    assert_eq!(dialogs.len(), 1, "one Dialog node: {dialogs:?}");
    assert_eq!(
        dialogs[0]["label"],
        label("shortcuts"),
        "named by its title"
    );
    // The Done button sits inside the dialog.
    let parent: std::collections::HashMap<String, String> = all
        .iter()
        .flat_map(|n| {
            n["children"].as_array().unwrap().iter().map(move |c| {
                (
                    c.as_str().unwrap().to_owned(),
                    n["id"].as_str().unwrap().to_owned(),
                )
            })
        })
        .collect();
    let done = all
        .iter()
        .find(|n| n["label"] == label("done") && n["role"] == "Button")
        .expect("a labelled Done button");
    let mut at = done["id"].as_str().unwrap().to_owned();
    let mut inside = false;
    while let Some(up) = parent.get(&at) {
        if Some(up.as_str()) == dialogs[0]["id"].as_str() {
            inside = true;
            break;
        }
        at = up.clone();
    }
    assert!(inside, "Done is a descendant of the Dialog node");

    send(&mut rig, 3, "busviewer.dialog", json!({"open":null}));
    wait(&mut rig, &[3]);
    let (_, rc, body) = reply(&rig, 3).unwrap();
    assert_eq!((rc, &body["dialog"]), (0, &Value::Null));
    assert_eq!(engine(&rig).ui.dialog, None, "closed");
    rig.h.run_steps(3);
    assert!(
        tree(&mut rig, 4, json!({"role":"Dialog"})).is_empty(),
        "no dialog node once closed"
    );
    let toggle = control(&mut rig, 5, &label("toggle-mode"));
    send(&mut rig, 6, "busviewer.ui.click", json!({"id":toggle}));
    wait(&mut rig, &[6]);
    rig.h.run_steps(2);
    assert_eq!(
        engine(&rig).ui.theme_mode,
        Some(design::Mode::Dark),
        "the title bar takes clicks again"
    );
}

/// The panel colour `scheme` in `mode` installs: what the window shows.
fn panel_of(scheme: design::Scheme, mode: design::Mode) -> egui::Color32 {
    let theme = Theme::for_context(design::DesignContext {
        scheme,
        mode,
        ..Default::default()
    });
    toolkit::style::style(&theme).visuals.panel_fill
}

/// View › Theme › Forest, clicked over the Bus as a person would, installs
/// Forest in the session's mode, and the menu ticks it.
#[test]
fn view_theme_forest_by_bus_clicks_installs_forest() {
    use design::{Mode, Scheme};
    let mut rig = live_rig();
    assert_eq!(
        rig.h.ctx.global_style().visuals.panel_fill,
        panel_of(Scheme::Pro, Mode::Light),
        "the session theme"
    );
    for (n, name) in [
        (1, label("view")),
        (2, "Theme".to_owned()),
        (3, "Forest".to_owned()),
    ] {
        send(&mut rig, n, "busviewer.ui.click", json!({"label":name}));
        wait(&mut rig, &[n]);
        let (_, rc, body) = reply(&rig, n).unwrap();
        assert_eq!(rc, 0, "{name}: {body}");
    }
    rig.h.run_steps(10);
    assert_eq!(
        (engine(&rig).ui.theme_scheme, engine(&rig).ui.theme_mode),
        (Some(Scheme::Forest), None)
    );
    assert_eq!(
        rig.h.ctx.global_style().visuals.panel_fill,
        panel_of(Scheme::Forest, Mode::Light),
        "Forest is installed"
    );
    send(&mut rig, 4, "busviewer.commands", json!({}));
    wait(&mut rig, &[4]);
    let commands = reply(&rig, 4).unwrap().2;
    let tick = |id: &str| {
        commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()["checked"]
            .clone()
    };
    assert_eq!(
        (
            tick("view.theme.forest"),
            tick("view.theme.session"),
            tick("bus.call")
        ),
        (json!(true), json!(false), Value::Null)
    );
}

/// busviewer.theme sets an axis, null follows the session again, an unknown
/// name is refused; the window installs each answer's effective theme.
#[test]
fn the_theme_verb_installs_its_choice() {
    use design::{Mode, Scheme};
    let mut rig = live_rig();
    send(
        &mut rig,
        1,
        "busviewer.theme",
        json!({"scheme":"studio","mode":"dark"}),
    );
    wait(&mut rig, &[1]);
    let (_, rc, body) = reply(&rig, 1).unwrap();
    assert_eq!(
        (rc, &body["effective"]),
        (0, &json!({"scheme":"studio","mode":"dark"})),
        "{body}"
    );
    rig.h.run_steps(5);
    assert_eq!(
        rig.h.ctx.global_style().visuals.panel_fill,
        panel_of(Scheme::Studio, Mode::Dark)
    );
    send(
        &mut rig,
        2,
        "busviewer.theme",
        json!({"scheme":null,"mode":null}),
    );
    wait(&mut rig, &[2]);
    let (_, rc, body) = reply(&rig, 2).unwrap();
    assert_eq!(
        (rc, &body["scheme"], &body["effective"]),
        (0, &Value::Null, &json!({"scheme":"pro","mode":"light"}))
    );
    rig.h.run_steps(5);
    assert_eq!(
        rig.h.ctx.global_style().visuals.panel_fill,
        panel_of(Scheme::Pro, Mode::Light),
        "back to the session theme"
    );
    send(&mut rig, 3, "busviewer.theme", json!({"scheme":"neon"}));
    wait(&mut rig, &[3]);
    assert_eq!(reply(&rig, 3).unwrap().2["error_code"], "ARGUMENT");
}

/// The one link named `name` exactly.
fn control_link(rig: &mut Rig, id: u64, name: &str) -> String {
    let nodes = tree(rig, id, json!({"label":name,"exact":true,"role":"Link"}));
    assert_eq!(nodes.len(), 1, "{name}: {nodes:?}");
    nodes[0]["id"].as_str().unwrap().to_owned()
}

/// Live bug 2: with the toggle's tooltip showing, the tooltip repeats its
/// label, and a click by label still names the button.
#[test]
fn a_click_by_label_prefers_the_control_over_its_tooltip() {
    let mut rig = live_rig();
    let name = label("toggle-mode");
    let toggle = control(&mut rig, 1, &name);
    let rect = tree(
        &mut rig,
        2,
        json!({"label":name,"exact":true,"role":"Button"}),
    )[0]["rect"]
        .clone();
    let (x, y) = (
        (rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.0,
        (rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.0,
    );
    send(
        &mut rig,
        3,
        "busviewer.ui.pointer",
        json!({"x":x,"y":y,"action":"move"}),
    );
    wait(&mut rig, &[3]);
    // Rest still past the tooltip delay (0.35 s at 60 passes a second).
    rig.h.run_steps(40);
    let named = tree(&mut rig, 4, json!({"label":name,"exact":true}));
    assert!(
        named.len() >= 2,
        "the tooltip shows the same label: {named:?}"
    );
    send(&mut rig, 5, "busviewer.ui.click", json!({"label":name}));
    wait(&mut rig, &[5]);
    let (_, rc, body) = reply(&rig, 5).unwrap();
    assert_eq!(rc, 0, "{body}");
    assert_eq!(
        body["target"]["id"].as_str(),
        Some(toggle.as_str()),
        "the button, not its tooltip"
    );
    rig.h.run_steps(6);
    assert_eq!(engine(&rig).ui.theme_mode, Some(design::Mode::Dark));
}

/// Live bug 3: each expander names its row, so it can be read and
/// clicked by label.
#[test]
fn tree_expanders_name_their_rows() {
    let mut rig = rig();
    let expand = busviewer::label_with("expand", &[("name", "example")]);
    assert_eq!(expand, "Expand example");
    let id = control(&mut rig, 1, &expand);
    let _ = id;
    send(&mut rig, 2, "busviewer.ui.click", json!({"label":expand}));
    wait(&mut rig, &[2]);
    rig.h.run_steps(3);
    assert!(
        engine(&rig).ui.expanded.contains("service:example"),
        "{:?}",
        engine(&rig).ui.expanded
    );
    let collapse = busviewer::label_with("collapse", &[("name", "example")]);
    control(&mut rig, 3, &collapse);
    assert!(tree(&mut rig, 4, json!({"label":expand,"exact":true})).is_empty());
}

/// Item 4: an exact label filter lists only nodes named exactly that.
#[test]
fn the_tree_filters_by_exact_label() {
    let mut rig = rig();
    let loose = tree(&mut rig, 1, json!({"label":"close"}));
    assert!(loose.iter().any(|n| n["label"] == "Close"), "{loose:?}");
    let exact = tree(&mut rig, 2, json!({"label":"Close","exact":true}));
    assert!(
        !exact.is_empty() && exact.iter().all(|n| n["label"] == "Close"),
        "{exact:?}"
    );
    assert!(
        tree(&mut rig, 3, json!({"label":"close","exact":true})).is_empty(),
        "exact is case-sensitive"
    );
    send(
        &mut rig,
        4,
        "busviewer.ui.tree",
        json!({"label":"Close","exact":"yes"}),
    );
    wait(&mut rig, &[4]);
    assert_eq!(reply(&rig, 4).unwrap().2["error_code"], "ARGUMENT");
}
