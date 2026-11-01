// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The pages over the Bus, on the real [`Shell`] with a logging handle: an
//! agent reads the tree and a page as data, opens pages, lands on rows,
//! clicks controls through `prefs.ui.*` as a hand would, and sets keys
//! with `prefs.set`; each write is one fenced `settings.apply`. A stale
//! mirror disables the rows before any click.
use citizen::{Delivery, Handle, Logged};
use egui_kittest::Harness;
use futures::channel::mpsc;
use prefs::shell::{APP, Shell};
use serde_json::{Value, json};
use std::time::Duration;
use toolkit::{Theme, drive};

struct Rig {
    h: Harness<'static, Option<Shell>>,
    bus: Handle,
    deliveries: mpsc::Sender<Delivery>,
}

/// COSMIC's default window, 1024 by 720.
fn rig() -> Rig {
    let theme = Theme::embedded();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1024.0, 720.0))
        .build_ui_state(
            |ui, shell: &mut Option<Shell>| {
                if let Some(shell) = shell {
                    shell.logic(ui.ctx());
                    shell.ui(ui);
                }
            },
            None,
        );
    toolkit::install(&h.ctx, &theme);
    let runtime = std::env::temp_dir().join(format!("prefs-pages-{}", std::process::id()));
    std::fs::create_dir_all(&runtime).unwrap();
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
    h.run_steps(3);
    Rig { h, bus, deliveries }
}

/// The store as settingsd at revision 7 answers it.
fn read(rig: &mut Rig, role: &str, stale: bool) {
    let engine = rig.h.state_mut().as_mut().unwrap().engine_mut();
    engine.store_read(&json!({"incarnation":"inc-1","revision":"7","values":{"appearance.density":"comfortable"}}));
    engine.store_status(&json!({"role": role, "stale": stale}));
    rig.h.run_steps(2);
}

fn send(rig: &mut Rig, id: u64, command_name: &str, args: Value) {
    rig.deliveries
        .try_send(Delivery::Command {
            id,
            command: command_name.into(),
            body: args.to_string(),
        })
        .unwrap();
}

fn reply(rig: &Rig, id: u64) -> Option<(u8, Value)> {
    rig.bus.log().into_iter().find_map(|l| match l {
        Logged::Reply { id: i, rc, body } if i == id => Some((rc, body)),
        _ => None,
    })
}

fn ask(rig: &mut Rig, id: u64, command_name: &str, args: Value) -> (u8, Value) {
    send(rig, id, command_name, args);
    for _ in 0..400 {
        rig.h.step();
        if let Some(answer) = reply(rig, id) {
            return answer;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{command_name} never answered: {:?}", rig.bus.log());
}

/// The `settings.apply` calls sent so far.
fn applies(rig: &Rig) -> Vec<Value> {
    rig.bus
        .log()
        .into_iter()
        .filter_map(|l| match l {
            Logged::Call {
                service,
                command,
                body,
            } if service == "settingsd" && command == "settings.apply" => {
                serde_json::from_str(&body).ok()
            }
            _ => None,
        })
        .collect()
}

/// The one node named `name` in `role`.
fn node(rig: &mut Rig, id: u64, name: &str, role: &str) -> String {
    let (rc, tree) = ask(
        rig,
        id,
        "prefs.ui.tree",
        json!({"label": name, "exact": true, "role": role}),
    );
    assert_eq!(rc, 0, "{tree}");
    let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
    assert_eq!(nodes.len(), 1, "{name}: {nodes:?}");
    nodes[0]["id"].as_str().unwrap().to_owned()
}

#[test]
fn the_tree_and_a_page_are_data() {
    let mut rig = rig();
    let (rc, tree) = ask(&mut rig, 1, "prefs.pages", json!({}));
    assert_eq!(rc, 0);
    let top: Vec<&str> = tree["pages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["parent"].is_null())
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(top.len(), 11, "{top:?}");
    assert_eq!(tree["open"], "desktop");
    let (rc, wifi) = ask(&mut rig, 2, "prefs.page", json!({"page":"wifi"}));
    assert_eq!(rc, 0);
    let toggle = &wifi["sections"][0]["rows"][0];
    assert_eq!(toggle["key"], "network.wifi.enabled");
    assert!(
        toggle["blocked"].as_str().unwrap().contains("hostd Wi-Fi"),
        "{toggle}"
    );
}

#[test]
fn a_deep_link_lands_on_a_drawer_row_and_back_goes_to_the_parent() {
    let mut rig = rig();
    let (rc, body) = ask(
        &mut rig,
        1,
        "prefs.page.open",
        json!({"page":"appearance","row":"frosted_opacity"}),
    );
    assert_eq!(rc, 0, "{body}");
    assert_eq!(body["result"]["drawer"], "frosted");
    let (_, info) = ask(&mut rig, 2, "prefs.info", json!({}));
    assert_eq!(info["nav"]["page"], "appearance");
    let (rc, _) = ask(&mut rig, 3, "prefs.execute", json!({"id":"page.back"}));
    assert_eq!(rc, 0);
    let (_, info) = ask(&mut rig, 4, "prefs.info", json!({}));
    assert_eq!(info["nav"]["page"], "desktop");
    let (rc, body) = ask(&mut rig, 5, "prefs.page.open", json!({"page":"nowhere"}));
    assert_eq!((rc, body["error_code"].as_str()), (10, Some("REFUSED")));
}

#[test]
fn a_bus_click_on_a_radio_row_applies_once_fenced() {
    let mut rig = rig();
    read(&mut rig, "authority", false);
    ask(
        &mut rig,
        1,
        "prefs.page.open",
        json!({"page":"appearance","row":"density"}),
    );
    rig.h.run_steps(3);
    let compact = node(&mut rig, 2, "Compact", "RadioButton");
    let (rc, body) = ask(&mut rig, 3, "prefs.ui.click", json!({"id": compact}));
    assert_eq!(rc, 0, "{body}");
    rig.h.run_steps(4);
    let sent = applies(&rig);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["expected_revision"], "7");
    assert_eq!(sent[0]["changes"], json!({"appearance.density":"compact"}));
}

#[test]
fn prefs_set_writes_a_key_and_refuses_live_and_unknown_ones() {
    let mut rig = rig();
    read(&mut rig, "authority", false);
    let (rc, body) = ask(
        &mut rig,
        1,
        "prefs.set",
        json!({"key":"shell.panels.bottom.autohide","value":true}),
    );
    assert_eq!(rc, 0, "{body}");
    assert_eq!(body["result"]["value"], true);
    rig.h.run_steps(3);
    assert_eq!(
        applies(&rig)[0]["changes"],
        json!({"shell.panels.bottom.autohide":true})
    );
    for (id, key) in [(2, "network.wifi.enabled"), (3, "no.such.key")] {
        let (rc, body) = ask(&mut rig, id, "prefs.set", json!({"key":key,"value":true}));
        assert_eq!(
            (rc, body["error_code"].as_str()),
            (10, Some("REFUSED")),
            "{key}"
        );
    }
}

#[test]
fn a_stale_mirror_disables_the_rows_before_any_click() {
    let mut rig = rig();
    read(&mut rig, "mirror", true);
    ask(
        &mut rig,
        1,
        "prefs.page.open",
        json!({"page":"appearance","row":"density"}),
    );
    let (_, page) = ask(&mut rig, 2, "prefs.page", json!({}));
    let density = page["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["rows"].as_array().unwrap().iter())
        .find(|r| r["id"] == "density")
        .unwrap()
        .clone();
    assert!(
        density["blocked"].as_str().unwrap().contains("mirror"),
        "{density}"
    );
    rig.h.run_steps(3);
    let compact = node(&mut rig, 3, "Compact", "RadioButton");
    ask(&mut rig, 4, "prefs.ui.click", json!({"id": compact}));
    rig.h.run_steps(4);
    assert!(applies(&rig).is_empty());
}

#[test]
fn search_lands_on_the_control() {
    let mut rig = rig();
    let (rc, body) = ask(&mut rig, 1, "prefs.search", json!({"query":"gaps"}));
    assert_eq!(rc, 0);
    assert_eq!(
        body["hits"][0],
        json!({"page":"appearance","section":"windows","row":"gaps","item":null,"title":"Gaps around tiled windows"})
    );
}

/// Desktop › Window management over the Bus: the page opens, a write to a
/// live key applies once and fenced, and the focus rows (no compd focus
/// policy yet) refuse writes.
#[test]
fn window_management_writes_a_live_key_once_and_refuses_the_missing_backend() {
    let mut rig = rig();
    read(&mut rig, "authority", false);
    let (rc, body) = ask(
        &mut rig,
        1,
        "prefs.page.open",
        json!({"page":"window_management"}),
    );
    assert_eq!(rc, 0, "{body}");
    rig.h.run_steps(3);
    let (rc, body) = ask(
        &mut rig,
        2,
        "prefs.set",
        json!({"key":"window.edge_gravity","value":true}),
    );
    assert_eq!(rc, 0, "{body}");
    rig.h.run_steps(3);
    let sent = applies(&rig);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["changes"], json!({"window.edge_gravity":true}));
    for (id, key, value) in [
        (3, "window.focus_follows_cursor", json!(true)),
        (4, "window.focus_delay_ms", json!(500)),
    ] {
        let (rc, body) = ask(&mut rig, id, "prefs.set", json!({"key":key,"value":value}));
        assert_eq!(
            (rc, body["error_code"].as_str()),
            (10, Some("REFUSED")),
            "{key}"
        );
    }
    rig.h.run_steps(3);
    assert_eq!(applies(&rig).len(), 1);
}

/// The write path end to end through the one dispatch a click and the Bus
/// share: three gestures make one apply and one merged second apply on the
/// receipt's revision; a conflict on that one reads and sends it again.
#[test]
fn gestures_merge_and_a_conflict_resends_through_the_dispatch() {
    use preferences::Effect;
    let commands = prefs::actions::registry();
    let mut e = preferences::Engine::new(prefs::label_with);
    e.take_effects();
    e.store_read(&json!({"incarnation":"inc-1","revision":"7","values":{}}));
    e.store_status(&json!({"role":"authority","stale":false}));
    let gesture = |e: &mut preferences::Engine, key: &str, value: Value| {
        commands
            .execute_with(
                "set",
                e,
                json!({"key":key,"value":value}),
                toolkit::action::Origin::Ui,
            )
            .unwrap();
    };
    let next = |e: &mut preferences::Engine| {
        e.take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Store {
                    ticket,
                    command,
                    body,
                } => Some((ticket, command, body)),
                _ => None,
            })
            .expect("a store call")
    };
    let ok = |body: Value| {
        Ok(citizen::Reply {
            rc: 0,
            body: body.to_string(),
            error: None,
        })
    };
    gesture(&mut e, "appearance.density", json!("compact"));
    let (t, command, body) = next(&mut e);
    assert_eq!(command, "settings.apply");
    assert_eq!(body["changes"], json!({"appearance.density":"compact"}));
    gesture(&mut e, "appearance.roundness", json!("square"));
    gesture(&mut e, "appearance.density", json!("spacious"));
    e.stored(
        t,
        ok(json!({"status":"changed","receipt":{"incarnation":"inc-1","revision":"8"}})),
    );
    let (t, command, _) = next(&mut e);
    assert_eq!(command, "settings.get");
    e.stored(
        t,
        ok(json!({"incarnation":"inc-1","revision":"8","values":{"appearance.density":"compact"}})),
    );
    let (t, command, body) = next(&mut e);
    assert_eq!(command, "settings.apply");
    assert_eq!(body["expected_revision"], "8");
    let merged = json!({"appearance.density":"spacious","appearance.roundness":"square"});
    assert_eq!(body["changes"], merged);
    e.stored(
        t,
        Ok(citizen::Reply {
            rc: 10,
            body: json!({"status":"conflict","revision":"9"}).to_string(),
            error: None,
        }),
    );
    let (t, command, _) = next(&mut e);
    assert_eq!(command, "settings.get");
    e.stored(
        t,
        ok(json!({"incarnation":"inc-1","revision":"9","values":{"appearance.density":"compact"}})),
    );
    let (_, command, body) = next(&mut e);
    assert_eq!(command, "settings.apply");
    assert_eq!(body["expected_revision"], "9");
    assert_eq!(body["changes"], merged);
}

/// The drawer `prefs.info` reports open.
fn open_drawer(rig: &mut Rig, id: u64) -> Value {
    let (_, info) = ask(rig, id, "prefs.info", json!({}));
    info["nav"]["drawer"].clone()
}

/// A click on a colour swatch, the accent's custom swatch and a pick row
/// opens that row's own drawer, as a page drawer's go-next row does.
#[test]
fn a_click_on_a_swatch_the_custom_accent_or_a_pick_row_opens_its_drawer() {
    let mut rig = rig();
    read(&mut rig, "authority", false);
    for (n, row, name, role) in [
        (10, "window_bg", "Window background", "Button"),
        (20, "accent", "Custom color", "Button"),
        (30, "font", "System font", "Link"),
        (40, "frosted", "Frosted glass", "Link"),
    ] {
        ask(
            &mut rig,
            n,
            "prefs.page.open",
            json!({"page":"appearance","row":row}),
        );
        rig.h.run_steps(4);
        let control = node(&mut rig, n + 1, name, role);
        let (rc, body) = ask(&mut rig, n + 2, "prefs.ui.click", json!({"id": control}));
        assert_eq!(rc, 0, "{name}: {body}");
        rig.h.run_steps(4);
        let expected = if row == "frosted" { "frosted" } else { row };
        assert_eq!(open_drawer(&mut rig, n + 3), json!(expected), "{name}");
    }
}
