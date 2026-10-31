// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed answers use the same path as the window buttons.
use crate::app::{Answer, App, Dialog};
use serde_json::{Value, json};

pub fn view(app: &App) -> Value {
    let Some(dialog) = &app.dialog else { return Value::Null };
    let (kind, title, buttons) = match dialog {
        Dialog::Confirm { .. } => ("delete", "confirm", vec![("yes", "yes", "default"), ("no", "no", "cancel")]),
        Dialog::Prompt { kind, .. } => ("name", match kind { dopus_core::PromptKind::NewFolder => "new-folder", dopus_core::PromptKind::Rename => "rename" }, vec![("ok", "ok", "default"), ("cancel", "cancel", "cancel")]),
        Dialog::Drop { .. } => ("drop", "drop-body", vec![("move", "move-here", "default"), ("copy", "copy-here", "other"), ("cancel", "cancel", "cancel")]),
        Dialog::Shortcuts => ("shortcuts", "shortcuts", vec![("done", "done", "default")]),
        Dialog::About => ("about", "about", vec![("done", "done", "default")]),
    };
    let mut value = json!({"kind":kind, "title":crate::label(title), "buttons":buttons.into_iter().map(|(id,label,role)| json!({"id":id,"label":crate::label(label),"role":role})).collect::<Vec<_>>(), "status":app.status});
    match dialog {
        Dialog::Confirm { message, .. } => value["body"] = json!(message),
        Dialog::Prompt { text, .. } => value["name"] = json!(text),
        Dialog::Drop { paths, destination, .. } => { value["paths"] = json!(paths); value["path"] = json!(destination); value["title"] = json!(destination.file_name().unwrap_or_default().to_string_lossy()); }
        _ => {}
    }
    value
}

pub fn command(app: &mut App, args: &Value) -> (u8, Value) {
    match apply(app, args) {
        Ok(()) => (0, json!({"dialog":view(app)})),
        Err((code, message)) => (10, json!({"error_code":code,"message":message})),
    }
}

fn apply(app: &mut App, args: &Value) -> Result<(), (&'static str, String)> {
    let bad = || ("ARGUMENT", "expected {}, {open: kind or null}, or {answer: button id, name?: string}".into());
    let map = args.as_object().ok_or_else(bad)?;
    if map.keys().any(|k| !["open", "answer", "name"].contains(&k.as_str())) || (map.contains_key("open") && map.len() != 1) || (map.contains_key("name") && !map.contains_key("answer")) { return Err(bad()); }
    if let Some(open) = map.get("open") {
        if open.is_null() { app.answer(Answer::Cancel); }
        else {
            let dialog = match open.as_str() { Some("shortcuts") => Dialog::Shortcuts, Some("about") => Dialog::About, _ => return Err(bad()) };
            if app.dialog.is_some() { return Err(("DIALOG_OPEN", "answer or close the open dialog first".into())); }
            app.open(dialog);
        }
    } else if let Some(answer) = map.get("answer") {
        if app.dialog.is_none() { return Err(("NO_DIALOG", "no dialog is open".into())); }
        let answer = answer.as_str().ok_or_else(bad)?;
        if !view(app)["buttons"].as_array().unwrap().iter().any(|b| b["id"] == answer) { return Err(bad()); }
        if let Some(name) = map.get("name") {
            let name = name.as_str().ok_or_else(bad)?;
            let text = app.prompt_text().ok_or_else(bad)?;
            *text = name.to_owned();
        }
        if answer == "ok" && let Some(text) = app.prompt_text() { dopus_core::validate_filename(text).map_err(|e| ("ARGUMENT", e))?; }
        app.answer(match answer { "yes" | "ok" | "move" | "done" => Answer::Accept, "copy" => Answer::Other, _ => Answer::Cancel });
    }
    Ok(())
}
