// SPDX-License-Identifier: MIT OR Apache-2.0
//! The real [`Shell`] on a logging Bus handle: what Prefs publishes on
//! `<service>.command.executed` (docs/decisions/2026-10-11-executed-commands.md).
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

fn send(rig: &mut Rig, id: u64, verb: &str, args: Value) {
    rig.deliveries
        .try_send(Delivery::Command {
            id,
            verb: verb.into(),
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
            Logged::Publish { topic, body } if topic == "sink.command.executed" => Some(body),
            _ => None,
        })
        .collect()
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
    rig.h.get_by_label("About Prefs").click();
    rig.h.run_steps(3);
    let bodies = published(&rig);
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
