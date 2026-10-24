// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The real [`Shell`] on a logging Bus handle: what Preferences publishes on
//! `<service>.action.executed` (docs/docs/decisions/2026-10-11-executed-commands.md).
use citizen::{Delivery, Handle, Logged};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

fn rig() -> Rig {
    let theme = Theme::embedded();
    let mut h = Harness::builder()
        .with_size(egui::vec2(980.0, 620.0))
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
    let runtime = std::env::temp_dir().join(format!("prefs-shell-{}", std::process::id()));
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

fn send(rig: &mut Rig, id: u64, command_name: &str, args: Value) {
    rig.deliveries
        .try_send(Delivery::Command {
            id,
            command: command_name.into(),
            body: args.to_string(),
        })
        .unwrap();
}

fn answered(rig: &Rig, id: u64) -> bool {
    rig.bus
        .log()
        .iter()
        .any(|l| matches!(l, Logged::Reply { id: i, .. } if *i == id))
}

fn wait(rig: &mut Rig, ids: &[u64]) {
    for _ in 0..400 {
        rig.h.step();
        if ids.iter().all(|id| answered(rig, *id)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("replies {ids:?} never came: {:?}", rig.bus.log());
}

fn published(rig: &Rig) -> Vec<Value> {
    rig.bus
        .log()
        .into_iter()
        .filter_map(|l| match l {
            Logged::Publish { topic, body } if topic == "sink.action.executed" => Some(body),
            _ => None,
        })
        .collect()
}

/// `prefs.email.password` hands the password to the email service in the
/// call's body, which only the shell fills in as it sends, and answers with
/// Email's outcome (here the logging handle sends nothing, so: not sent).
/// The answer never repeats the password.
#[test]
fn a_password_goes_to_email_and_is_answered_without_it() {
    let mut rig = rig();
    send(
        &mut rig,
        7,
        "prefs.email.password",
        json!({"id": "acct-1", "password": "hunter2"}),
    );
    wait(&mut rig, &[7]);
    let log = rig.bus.log();
    let call = log
        .iter()
        .find_map(|l| match l {
            Logged::Call {
                service,
                command: command_name,
                body,
            } if command_name == "email.account.password.set" => Some((service.clone(), body.clone())),
            _ => None,
        })
        .expect("the call to Email");
    assert_eq!(call.0, "email");
    let body: Value = serde_json::from_str(&call.1).unwrap();
    assert_eq!(body, json!({"id": "acct-1", "password": "hunter2"}));
    let (rc, reply) = log
        .iter()
        .find_map(|l| match l {
            Logged::Reply { id: 7, rc, body } => Some((*rc, body.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(rc, 10);
    assert_eq!(reply["error_code"], "NOT_SENT");
    assert!(!reply.to_string().contains("hunter2"));
}

/// A Bus execute and a menu pick are published with how they were reached
/// and consecutive seq; an unknown command publishes nothing.
#[test]
fn executed_commands_are_published_with_their_origin() {
    let mut rig = rig();
    send(&mut rig, 1, "prefs.execute", json!({"id": "nope"}));
    send(
        &mut rig,
        2,
        "prefs.execute",
        json!({"id": "help.shortcuts"}),
    );
    wait(&mut rig, &[1, 2]);
    rig.h.key_press(egui::Key::Escape);
    rig.h.run_steps(3);
    rig.h.get_by_label("Help").click();
    rig.h.run_steps(3);
    rig.h.get_by_label("About Preferences").click();
    rig.h.run_steps(3);
    let bodies = published(&rig);
    for body in &bodies {
        assert_eq!(body["v"], 2);
        assert_eq!(body.get("params"), Some(&serde_json::Value::Null));
        assert_eq!(body.get("result"), Some(&serde_json::Value::Null));
    }
    let got: Vec<(&str, &str)> = bodies
        .iter()
        .map(|b| (b["id"].as_str().unwrap(), b["origin"].as_str().unwrap()))
        .collect();
    assert_eq!(
        got,
        vec![("help.shortcuts", "bus"), ("help.about", "menu")],
        "{:?}",
        rig.bus.log()
    );
    assert_eq!(
        bodies[1]["seq"].as_u64(),
        bodies[0]["seq"].as_u64().map(|s| s + 1)
    );
}

#[test]
fn slash_focuses_search_enter_opens_first_match_and_escape_clears_and_unfocuses() {
    use egui::accesskit::Role;
    let mut rig = rig();
    let search = egui::Id::new("preferences-search");
    rig.h.key_press(egui::Key::Slash);
    rig.h.run_steps(3);
    assert!(rig.h.ctx.memory(|m| m.has_focus(search)));
    rig.h.key_press(egui::Key::Slash);
    rig.h.run_steps(3);
    assert_eq!(
        published(&rig)
            .iter()
            .filter(|body| body["id"] == "view.search")
            .count(),
        1,
        "a text field keeps slash while typing"
    );
    rig.h.get_by_role(Role::TextInput).type_text("background");
    rig.h.run_steps(3);
    rig.h.key_press(egui::Key::Enter);
    rig.h.run_steps(3);
    send(&mut rig, 11, "prefs.info", json!({}));
    wait(&mut rig, &[11]);
    assert!(
        rig.bus.log().iter().any(|entry| matches!(entry,
            Logged::Reply { id: 11, rc: 0, body }
                if body["section"] == "wallpapers" && body["category"] == "appearance"
        )),
        "{:?}",
        rig.bus.log()
    );
    rig.h.key_press(egui::Key::Escape);
    rig.h.run_steps(3);
    assert!(!rig.h.ctx.memory(|m| m.has_focus(search)));
    send(&mut rig, 12, "prefs.info", json!({}));
    wait(&mut rig, &[12]);
    assert!(rig.bus.log().iter().any(|entry| matches!(entry,
        Logged::Reply { id: 12, rc: 0, body } if body["ui"]["search"] == ""
    )));
}

#[test]
fn search_command_uses_translated_titles_and_leaves_the_window_alone() {
    let mut rig = rig();
    send(&mut rig, 21, "prefs.search", json!({"query":"APPS"}));
    send(&mut rig, 22, "prefs.info", json!({}));
    wait(&mut rig, &[21, 22]);
    assert!(rig.bus.log().iter().any(|entry| matches!(entry,
        Logged::Reply { id: 21, rc: 0, body } if body["matches"] == json!([
            {"id":"email","category":"apps"},
            {"id":"terminal","category":"apps"},
            {"id":"text","category":"apps"}
        ])
    )));
    assert!(rig.bus.log().iter().any(|entry| matches!(entry,
        Logged::Reply { id: 22, rc: 0, body }
            if body["section"] == "software" && body["ui"]["search"] == ""
    )));
}

#[test]
fn every_registry_section_opens_through_the_shell() {
    let mut rig = rig();
    for (index, section) in preferences::SECTIONS.iter().enumerate() {
        let id = 100 + index as u64;
        send(&mut rig, id, "prefs.section", json!({"name":section.id}));
        wait(&mut rig, &[id]);
        assert!(
            rig.bus.log().iter().any(|entry| matches!(entry,
                Logged::Reply { id: reply_id, rc: 0, body } if *reply_id == id
                    && body["section"] == section.id
                    && body["category"] == section.category.name()
            )),
            "{}: {:?}",
            section.id,
            rig.bus.log()
        );
    }
    send(&mut rig, 200, "prefs.search", json!({"query":"compact"}));
    send(&mut rig, 201, "prefs.search", json!({"query":"animation"}));
    wait(&mut rig, &[200, 201]);
    for (id, section) in [(200, "size"), (201, "motion")] {
        assert!(rig.bus.log().iter().any(|entry| matches!(entry,
            Logged::Reply { id: reply_id, rc: 0, body } if *reply_id == id
                && body["matches"] == json!([{"id":section,"category":"appearance"}])
        )));
    }
}
