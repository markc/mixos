// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `screenshot.v1` Bus surface: every command the app answers itself, with
//! its arguments, as `HELP` and `app.describe` report it. The window adds
//! the toolkit's drive commands (`screenshot.ui.*`, `screenshot.window*`).
//!
//! Ported from Cap's commands (`cap.*`, markc/mixos-iced), renamed, with the
//! agent commands for the screenshots folder (`save`, `recent`), the
//! person-intent capture (`take`) and the command registry (`commands`,
//! `execute`) added.
use serde_json::{Value, json};

/// The schema every reply's `schema` names.
pub const SCHEMA: &str = "screenshot.v1";

/// Every command the app answers itself, in description order.
pub const COMMANDS: [&str; 36] = [
    "HELP",
    "app.describe",
    "screenshot.ping",
    "screenshot.info",
    "screenshot.capture",
    "screenshot.take",
    "screenshot.cancel",
    "screenshot.open",
    "screenshot.annotate",
    "screenshot.move",
    "screenshot.delete",
    "screenshot.crop",
    "screenshot.undo",
    "screenshot.redo",
    "screenshot.export",
    "screenshot.save",
    "screenshot.recent",
    "screenshot.select",
    "screenshot.actions",
    "screenshot.execute",
    "screenshot.show",
    "screenshot.quit",
    "screenshot.theme",
    "screenshot.dialog",
    "screenshot.macros.list",
    "screenshot.macros.reload",
    "screenshot.macros.run",
    "screenshot.macros.status",
    "screenshot.macros.output",
    "screenshot.macros.stop",
    "screenshot.macros.new",
    "screenshot.macros.get",
    "screenshot.macros.put",
    "screenshot.macros.remove",
    "screenshot.macros.move",
    "screenshot.macros.copy",
];

/// Commands that change the document (refused under a dialog or a job).
pub fn is_edit(command_name: &str) -> bool {
    matches!(
        command_name,
        "screenshot.annotate"
            | "screenshot.move"
            | "screenshot.delete"
            | "screenshot.crop"
            | "screenshot.undo"
            | "screenshot.redo"
    )
}

/// Commands that take an empty object.
pub fn takes_nothing(command_name: &str) -> bool {
    matches!(
        command_name,
        "screenshot.ping"
            | "screenshot.info"
            | "screenshot.cancel"
            | "screenshot.undo"
            | "screenshot.redo"
            | "screenshot.save"
            | "screenshot.actions"
            | "screenshot.show"
            | "screenshot.quit"
    )
}

/// The app's own surface (the window adds the drive commands).
pub fn describe() -> Value {
    let shape = "{kind: arrow|line|rectangle|ellipse|pen|highlighter|redact|text|number, points: [{x,y}…] in original pixels (2, or 2..16384 for pen and highlighter), colour: [r,g,b,a], width: 0.5..512 original pixels, text?: string (text), size?: 8..1024 original pixels (text, number), number?: 1..9999 (number)}";
    let mut surface = json!({"schema": SCHEMA, "app_id": crate::APP_ID, "commands": [
        {"name":"HELP","description":"List every command","read_only":true},
        {"name":"app.describe","description":"Describe the app and every command with its arguments","read_only":true},
        {"name":"screenshot.ping","description":"Probe the app","read_only":true},
        {"name":"screenshot.info","description":"The compositor served, the document, its source and last save, the capture, the job and the UI state","read_only":true},
        {"name":"screenshot.capture","args":{"mode":"screen|window|region (default screen)","output":"optional output name","window":"{id, generation} (window mode only)","cursor":"bool, include the pointer (default false)","delay":"0..10 seconds","region":"optional {x, y, width, height, output?} in output-local logical pixels (region mode only): captured as given, with no drag"},"description":"Capture into a new document and answer when it is ready (restore_failed: true if the window could not be shown again); refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.take","args":{"mode":"optional screen|window|region (default: the window's)","window":"optional {id, generation}","region":"optional {x, y, width, height, output?} in output-local logical pixels (region mode only): captured as given, with no drag","comp":"optional: the compositor the caller expects; refused (COMPOSITOR) if this instance serves another"},"description":"Take a screenshot as Print Screen does: unsaved work is asked about in the window, never discarded; answers at once whether it started or is asking","read_only":false},
        {"name":"screenshot.cancel","description":"Cancel a capture in progress (during its delay, before its frame)","read_only":false},
        {"name":"screenshot.open","args":{"path":"image file"},"description":"Open an image as a new document; refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.annotate","args":shape,"deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.annotate: Add an annotation; answers its id","read_only":false},
        {"name":"screenshot.move","args":{"id":"annotation id","dx":"pixels","dy":"pixels"},"deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.move: Move an annotation","read_only":false},
        {"name":"screenshot.delete","args":{"id":"annotation id"},"deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.delete: Delete an annotation","read_only":false},
        {"name":"screenshot.crop","args":{"crop":"{x,y,width,height} in original pixels, or null to reset"},"deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.crop: Crop the export (the original is kept)","read_only":false},
        {"name":"screenshot.undo","deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.undo: Undo the last edit","read_only":false},
        {"name":"screenshot.redo","deprecated":true,"replaced_by":"screenshot.execute","description":"Deprecated alias of screenshot.execute edit.redo: Redo the last undone edit","read_only":false},
        {"name":"screenshot.export","args":{"path":"new PNG file"},"description":"Save As: publish the annotated PNG at a path that does not exist yet","read_only":false},
        {"name":"screenshot.save","description":"Save into the screenshots folder (Pictures/Screenshots) as YYYYMMDD_HHMMSS.png, local time (_2, _3... when taken); publishes screenshot.saved","read_only":false},
        {"name":"screenshot.recent","args":{"limit":"optional 1..100 (default 10)"},"description":"The most recent saved shots, newest first: path, width, height, annotated, saved_at","read_only":true},
        {"name":"screenshot.select","args":{"tool":"optional tool (select, crop, or an annotation kind)","window":"optional {id, generation} for window mode","output":"optional output name","mode":"optional screen|window|region"},"description":"Choose the tool, the capture mode, window or output, as the window's controls do","read_only":false},
        {"name":"screenshot.actions","description":"List the app's commands with labels, shortcuts, enablement and ticks","read_only":true},
        {"name":"screenshot.execute","args":{"id":"command id","params":"optional command parameters"},"description":"Run one command with parameters and return its result, through the same registry as the window","read_only":false},
        {"name":"screenshot.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"screenshot.quit","description":"Close; refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.theme","args":{"scheme":"optional scheme name or null to follow the session","mode":"optional light, dark or null"},"description":"Choose the window's scheme and mode; answers the choice","read_only":false},
        {"name":"screenshot.dialog","args":{"open":"optional \"about\", \"shortcuts\" or null (close as Escape does)","answer":"optional button id of the open dialog (discard: save, discard, keep; open: open, cancel; save_as: save, cancel; shortcuts, about: done)","path":"optional, with answer: fill the Open or Save As path first"},"description":"Read the open dialog ({} answers {dialog: {kind, title, buttons, body?, path?, status?} or null}), open or close a help dialog, or press one of its buttons; answered while a dialog is open","read_only":false}
    ]});
    surface["commands"]
        .as_array_mut()
        .unwrap()
        .extend(citizen::macros::describe("screenshot"));
    surface
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_described_once_in_order() {
        let described: Vec<String> = describe()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, COMMANDS);
    }

    #[test]
    fn edit_aliases_have_boolean_deprecation_and_a_replacement() {
        let surface = describe();
        let aliases: Vec<_> = surface["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| is_edit(v["name"].as_str().unwrap()))
            .collect();
        assert_eq!(aliases.len(), 6);
        for command_name in aliases {
            assert_eq!(command_name["deprecated"], true);
            assert_eq!(command_name["replaced_by"], "screenshot.execute");
        }
    }
}
