// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `screenshot.v1` Bus surface: every verb the app answers itself, with
//! its arguments, as `HELP` and `app.describe` report it. The window adds
//! the toolkit's drive verbs (`screenshot.ui.*`, `screenshot.window*`).
//!
//! Ported from Cap's verbs (`cap.*`, markc/mixos-iced), renamed, with the
//! agent verbs for the screenshots folder (`save`, `recent`), the
//! person-intent capture (`take`) and the command registry (`commands`,
//! `execute`) added.
use serde_json::{Value, json};

/// The schema every reply's `schema` names.
pub const SCHEMA: &str = "screenshot.v1";

/// Every verb the app answers itself, in description order.
pub const VERBS: [&str; 23] = [
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
    "screenshot.commands",
    "screenshot.execute",
    "screenshot.show",
    "screenshot.quit",
    "screenshot.theme",
];

/// Verbs that change the document (refused under a dialog or a job).
pub fn is_edit(verb: &str) -> bool {
    matches!(
        verb,
        "screenshot.annotate"
            | "screenshot.move"
            | "screenshot.delete"
            | "screenshot.crop"
            | "screenshot.undo"
            | "screenshot.redo"
    )
}

/// Verbs that take an empty object.
pub fn takes_nothing(verb: &str) -> bool {
    matches!(
        verb,
        "screenshot.ping"
            | "screenshot.info"
            | "screenshot.cancel"
            | "screenshot.undo"
            | "screenshot.redo"
            | "screenshot.save"
            | "screenshot.commands"
            | "screenshot.show"
            | "screenshot.quit"
    )
}

/// The app's own surface (the window adds the drive verbs).
pub fn describe() -> Value {
    let shape = "{kind: arrow|line|rectangle|ellipse|pen|highlighter|redact|text|number, points: [{x,y}…] in original pixels (2, or 2..16384 for pen and highlighter), colour: [r,g,b,a], width: 0.5..512 original pixels, text?: string (text), size?: 8..1024 original pixels (text, number), number?: 1..9999 (number)}";
    json!({"schema": SCHEMA, "app_id": crate::APP_ID, "verbs": [
        {"name":"HELP","description":"List every verb","read_only":true},
        {"name":"app.describe","description":"Describe the app and every verb with its arguments","read_only":true},
        {"name":"screenshot.ping","description":"Probe the app","read_only":true},
        {"name":"screenshot.info","description":"The compositor served, the document, its source and last save, the capture, the job and the UI state","read_only":true},
        {"name":"screenshot.capture","args":{"mode":"screen|window|region (default screen)","output":"optional output name","window":"{id, generation} (window mode only)","cursor":"bool, include the pointer (default false)","delay":"0..10 seconds"},"description":"Capture into a new document and answer when it is ready (restore_failed: true if the window could not be shown again); refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.take","args":{"mode":"optional screen|window|region (default: the window's)","window":"optional {id, generation}","comp":"optional: the compositor the caller expects; refused (COMPOSITOR) if this instance serves another"},"description":"Take a screenshot as Print Screen does: unsaved work is asked about in the window, never discarded; answers at once whether it started or is asking","read_only":false},
        {"name":"screenshot.cancel","description":"Cancel a capture in progress (during its delay, before its frame)","read_only":false},
        {"name":"screenshot.open","args":{"path":"image file"},"description":"Open an image as a new document; refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.annotate","args":shape,"description":"Add an annotation; answers its id","read_only":false},
        {"name":"screenshot.move","args":{"id":"annotation id","dx":"pixels","dy":"pixels"},"description":"Move an annotation","read_only":false},
        {"name":"screenshot.delete","args":{"id":"annotation id"},"description":"Delete an annotation","read_only":false},
        {"name":"screenshot.crop","args":{"crop":"{x,y,width,height} in original pixels, or null to reset"},"description":"Crop the export (the original is kept)","read_only":false},
        {"name":"screenshot.undo","description":"Undo the last edit","read_only":false},
        {"name":"screenshot.redo","description":"Redo the last undone edit","read_only":false},
        {"name":"screenshot.export","args":{"path":"new PNG file"},"description":"Save As: publish the annotated PNG at a path that does not exist yet","read_only":false},
        {"name":"screenshot.save","description":"Save into the screenshots folder (Pictures/Screenshots) as YYYYMMDD_HHMMSS.png, local time (_2, _3... when taken); publishes screenshot.saved","read_only":false},
        {"name":"screenshot.recent","args":{"limit":"optional 1..100 (default 10)"},"description":"The most recent saved shots, newest first: path, width, height, annotated, saved_at","read_only":true},
        {"name":"screenshot.select","args":{"tool":"optional tool (select, crop, or an annotation kind)","window":"optional {id, generation} for window mode","output":"optional output name","mode":"optional screen|window|region"},"description":"Choose the tool, the capture mode, window or output, as the window's controls do","read_only":false},
        {"name":"screenshot.commands","description":"List the app's commands with labels, shortcuts, enablement and ticks","read_only":true},
        {"name":"screenshot.execute","args":{"id":"command id"},"description":"Run one command by id, as its menu item or shortcut would","read_only":false},
        {"name":"screenshot.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"screenshot.quit","description":"Close; refused while unsaved annotations exist","read_only":false},
        {"name":"screenshot.theme","args":{"scheme":"optional scheme name or null to follow the session","mode":"optional light, dark or null"},"description":"Choose the window's scheme and mode; answers the choice","read_only":false}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_is_described_once_in_order() {
        let described: Vec<String> = describe()["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, VERBS);
    }
}
