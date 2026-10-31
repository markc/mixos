// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::app::{Answer, App, Dialog};
use serde_json::{Value, json};

pub fn view(app: &App) -> Value {
    let Some(dialog) = &app.dialog else {
        return Value::Null;
    };
    let (kind, title, buttons) = match dialog {
        Dialog::Recovered { .. } => (
            "recovered",
            "recovered-title",
            vec![
                ("open", "open-recovered", "default"),
                ("discard", "discard-recovered", "other"),
                ("done", "close", "cancel"),
            ],
        ),
        Dialog::Keys => ("keys", "keys-title", vec![("done", "close", "default")]),
        Dialog::About => ("about", "about-title", vec![("done", "close", "default")]),
        Dialog::Open { .. } => (
            "open",
            "open-title",
            vec![("open", "open", "default"), ("cancel", "cancel", "cancel")],
        ),
        Dialog::SaveAs { .. } => (
            "save_as",
            "save-as-title",
            vec![("save", "save", "default"), ("cancel", "cancel", "cancel")],
        ),
        Dialog::Goto { .. } => (
            "goto",
            "goto-title",
            vec![("go", "go", "default"), ("cancel", "cancel", "cancel")],
        ),
        Dialog::CloseDirty { .. } => (
            "close_dirty",
            "close-dirty-title",
            vec![
                ("save", "save", "default"),
                ("discard", "discard", "other"),
                ("cancel", "cancel", "cancel"),
            ],
        ),
        Dialog::DiskModified { .. } => (
            "disk_modified",
            "disk-modified-title",
            vec![("save", "save", "default"), ("cancel", "cancel", "cancel")],
        ),
    };
    let mut value = json!({"kind":kind,"title":crate::label(title),"buttons":buttons.into_iter().map(|(id,label,role)| json!({"id":id,"label":crate::label(label),"role":role})).collect::<Vec<_>>()});
    match dialog {
        Dialog::Recovered { rows } => {
            value["body"] = json!(crate::label("recovered-body"));
            value["rows"] = json!(
                rows.iter()
                    .map(|r| json!({"buffer":r.buffer,"path":r.path,"name":r.name,"bytes":r.bytes}))
                    .collect::<Vec<_>>()
            );
        }
        Dialog::Open { path, .. } | Dialog::SaveAs { path, .. } => value["path"] = json!(path),
        Dialog::Goto { text, .. } => value["text"] = json!(text),
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

fn apply(app: &mut App, args: &Value) -> Result<(), (&'static str, &'static str)> {
    let bad = || ("ARGUMENT", "expected open or a valid answer and its fields");
    let map = args.as_object().ok_or_else(bad)?;
    if map
        .keys()
        .any(|k| !["open", "answer", "buffer", "path", "text"].contains(&k.as_str()))
        || (map.contains_key("open") && map.len() != 1)
        || (!map.contains_key("open") && !map.contains_key("answer") && !map.is_empty())
    {
        return Err(bad());
    }
    if let Some(open) = map.get("open") {
        if open.is_null() {
            app.answer(Answer::Cancel);
        } else {
            let dialog = match open.as_str() {
                Some("keys") => Dialog::Keys,
                Some("about") => Dialog::About,
                _ => return Err(bad()),
            };
            if app.dialog.is_some() {
                return Err(("DIALOG_OPEN", "close the open dialog first"));
            }
            app.dialog = Some(dialog);
        }
    } else if let Some(answer) = map.get("answer") {
        let dialog = view(app);
        if dialog.is_null() {
            return Err(("NO_DIALOG", "no dialog is open"));
        }
        let answer = answer.as_str().ok_or_else(bad)?;
        if !dialog["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["id"] == answer)
        {
            return Err(bad());
        }
        let choice = if dialog["kind"] == "recovered" && matches!(answer, "open" | "discard") {
            let buffer = args["buffer"].as_str().ok_or_else(bad)?;
            if !dialog["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["buffer"] == buffer)
            {
                return Err(bad());
            }
            Answer::Recovered {
                buffer: buffer.to_owned(),
                open: answer == "open",
            }
        } else {
            match answer {
                "open" | "save" | "go" | "done" => Answer::Accept,
                "discard" => Answer::Other,
                _ => Answer::Cancel,
            }
        };
        for field in ["path", "text"] {
            if let Some(value) = map.get(field) {
                let value = value.as_str().ok_or_else(bad)?;
                *app.dialog_text().ok_or_else(bad)? = value.to_owned();
            }
        }
        app.answer(choice);
    }
    Ok(())
}
