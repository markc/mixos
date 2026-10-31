// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::state::{Confirm, Dialog, DraftChoice, State};
use crate::view::UiEvent;
use serde_json::{Value, json};

pub fn view(state: &State) -> Value {
    let restore = state.compose.is_none() && state.form.is_none() && !state.restore.is_empty();
    let (kind, title, buttons) = if let Some(confirm) = state.confirm {
        (
            if confirm == Confirm::Quit {
                "quit"
            } else {
                "close_compose"
            },
            if confirm == Confirm::Quit {
                "quit-draft-title"
            } else {
                "close-draft-title"
            },
            vec![
                ("keep", "keep-draft", "default"),
                ("discard", "discard-draft", "other"),
                (
                    "cancel",
                    if confirm == Confirm::Quit {
                        "cancel"
                    } else {
                        "keep-editing"
                    },
                    "cancel",
                ),
            ],
        )
    } else if restore {
        (
            "draft_restore",
            "restore-title",
            vec![
                ("restore", "restore-draft", "default"),
                ("discard", "discard-draft", "other"),
                ("later", "later", "cancel"),
            ],
        )
    } else if let Some(dialog) = state.ui.dialog {
        match dialog {
            Dialog::About => ("about", "about", vec![("done", "done", "default")]),
            Dialog::Shortcuts => ("shortcuts", "shortcuts", vec![("done", "done", "default")]),
        }
    } else {
        return Value::Null;
    };
    let mut value = json!({"kind":kind,"title":crate::label(title),"buttons":buttons.into_iter().map(|(id,label,role)| json!({"id":id,"label":crate::label(label),"role":role})).collect::<Vec<_>>(),"status":state.status});
    if state.confirm.is_some() {
        value["body"] = json!(crate::label("close-draft-body"));
    } else if restore {
        value["body"] = json!(crate::label("restore-body"));
        value["draft"] = json!(state.restore.first());
    } else if kind == "about" {
        value["body"] = json!(crate::label("about-body"));
    }
    value
}

pub fn command(state: &mut State, args: &Value) -> (u8, Value) {
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
    let dialog = view(state);
    let kind = dialog["kind"].as_str().unwrap_or("");
    let choice = if let Some(open) = map.get("open") {
        if open.is_null() {
            Some("cancel")
        } else {
            let help = match open.as_str() {
                Some("about") => Dialog::About,
                Some("shortcuts") => Dialog::Shortcuts,
                _ => return error("ARGUMENT", "open must be about, shortcuts or null"),
            };
            if !dialog.is_null() {
                return error("DIALOG_OPEN", "close the open dialog first");
            }
            state.open_dialog(help);
            None
        }
    } else if let Some(answer) = map.get("answer") {
        if dialog.is_null() {
            return error("NO_DIALOG", "no dialog is open");
        }
        let Some(answer) = answer.as_str() else {
            return error("ARGUMENT", "answer must be a button id");
        };
        if !dialog["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["id"] == answer)
        {
            return error("ARGUMENT", "unknown button id");
        }
        Some(answer)
    } else {
        None
    };
    if let Some(answer) = choice {
        let choice = match answer {
            "keep" | "restore" => DraftChoice::Keep,
            "discard" => DraftChoice::Discard,
            _ => DraftChoice::Back,
        };
        match kind {
            "close_compose" | "quit" => state.apply_ui(UiEvent::ConfirmDraft(choice)),
            "draft_restore" => state.apply_ui(UiEvent::RestoreDraft(choice)),
            _ => state.apply_ui(UiEvent::CloseDialog),
        }
    }
    (0, json!({"dialog":view(state)}))
}
