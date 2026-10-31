// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::app::App;
use serde_json::{Value, json};

pub fn command(app: &mut App, args: &Value) -> (u8, Value) {
    let error = |code, message| (10, json!({"error_code":code,"message":message}));
    let Some(map) = args.as_object() else {
        return error("ARGUMENT", "expected an object");
    };
    if map.len() > 1
        || map
            .keys()
            .any(|k| !["open", "answer"].contains(&k.as_str()))
    {
        return error("ARGUMENT", "expected open or answer");
    }
    if let Some(open) = map.get("open") {
        if open.is_null() {
            app.keys_dialog = false;
            app.about_dialog = false;
        } else {
            if !matches!(open.as_str(), Some("keys" | "about")) {
                return error("ARGUMENT", "open must be keys, about or null");
            }
            if app.keys_dialog || app.about_dialog {
                return error("DIALOG_OPEN", "close the open dialog first");
            }
            app.keys_dialog = open == "keys";
            app.about_dialog = open == "about";
        }
    } else if let Some(answer) = map.get("answer") {
        if !app.keys_dialog && !app.about_dialog {
            return error("NO_DIALOG", "no dialog is open");
        }
        if answer != "done" {
            return error("ARGUMENT", "the dialog has only done");
        }
        app.keys_dialog = false;
        app.about_dialog = false;
    }
    let dialog = if app.keys_dialog {
        Some(("keys", "keys-title"))
    } else if app.about_dialog {
        Some(("about", "about-title"))
    } else {
        None
    };
    (
        0,
        json!({"dialog":dialog.map(|(kind,title)| json!({"kind":kind,"title":crate::label(title),"buttons":[{"id":"done","label":crate::label("close"),"role":"default"}]}))}),
    )
}
