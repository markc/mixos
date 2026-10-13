// SPDX-License-Identifier: MIT OR Apache-2.0

//! The wallpaper editor: a pure model of the backdrop wallpaper config that
//! the prefs view edits. It plans Bus calls and folds their replies back in.
//! It does no I/O: the coordinator sends each `Call` and feeds the answer to
//! `complete`.

use std::collections::{BTreeMap, BTreeSet};

use citizen::{CallError, Reply};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SCHEMA: &str = "backdrop.v1";
const VERB_STATE: &str = "backdrop.state";
const VERB_SET: &str = "backdrop.set";
const MAX_OUTPUTS: usize = 64;
const MAX_IMAGES: usize = 4;
const MAX_DOCUMENT: usize = 64 * 1024;
const MAX_PATH: usize = 4096;

/// One Bus call to send. The coordinator answers it with `complete`.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub ticket: u64,
    pub verb: &'static str,
    pub body: Value,
}

/// The kind of wallpaper an edit chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Theme,
    Colour,
    Image,
}

/// One fill as the config file writes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback: Option<String>,
}

/// The schema 1 config. Unknown keys refuse editing; they are never dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<Fill>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    outputs: BTreeMap<String, Fill>,
}

impl Document {
    fn blank() -> Self {
        Self {
            schema: 1,
            default: None,
            outputs: BTreeMap::new(),
        }
    }

    /// The fill the target names itself. An output with no override is `None`.
    fn fill(&self, target: Option<&str>) -> Option<Fill> {
        match target {
            None => self.default.clone(),
            Some(name) => self.outputs.get(name).cloned(),
        }
    }

    /// Write one target's fill. `None` removes it: the default falls back to
    /// the theme, an output inherits the default.
    fn set_fill(&mut self, target: Option<&str>, fill: Option<Fill>) {
        match (target, fill) {
            (None, fill) => self.default = fill,
            (Some(name), Some(fill)) => {
                self.outputs.insert(name.to_owned(), fill);
            }
            (Some(name), None) => {
                self.outputs.remove(name);
            }
        }
    }

    fn check(&self) -> Result<(), &'static str> {
        if self.outputs.len() > MAX_OUTPUTS {
            return Err("status.too_many_outputs");
        }
        let images = self
            .default
            .iter()
            .chain(self.outputs.values())
            .filter_map(|fill| fill.image.as_deref())
            .collect::<BTreeSet<_>>();
        if images.len() > MAX_IMAGES {
            return Err("status.too_many_images");
        }
        let size = serde_json::to_vec(self)
            .map_err(|_| "status.too_large")?
            .len();
        if size > MAX_DOCUMENT {
            return Err("status.too_large");
        }
        for name in self.outputs.keys() {
            if name.is_empty() || name.len() > 64 || name == "." || name == ".." || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-'|b'_'|b'.')) {
                return Err("status.unknown_config");
            }
        }
        for fill in self.default.iter().chain(self.outputs.values()) {
            match (&fill.colour, &fill.image) {
                (Some(colour), None) if valid_colour(colour) && fill.fallback.is_none() => {},
                (None, Some(image)) if valid_path(image) && fill.fallback.as_deref().is_none_or(valid_colour) => {},
                _ => return Err("status.unknown_config"),
            }
        }
        Ok(())
    }
}

fn parse(value: Value) -> Result<Document, &'static str> {
    let document: Document =
        serde_json::from_value(value).map_err(|_| "status.unknown_config")?;
    if document.schema == 1 {
        document.check()?;
        Ok(document)
    } else {
        Err("status.unknown_config")
    }
}

fn valid_colour(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_path(text: &str) -> bool {
    text.starts_with('/') && !text.contains('\0') && text.len() <= MAX_PATH
}

/// The wallpaper editor. The public fields are what the view shows; the
/// private fields drive the Bus protocol.
#[derive(Debug, Clone, Serialize)]
pub struct WallpaperEditor {
    /// The whole schema 1 config as the daemon last reported it.
    pub current: Option<Value>,
    /// The config being edited. `None` when there is nothing to apply.
    pub draft: Option<Value>,
    /// `None` edits the default; `Some` edits that output's override.
    pub target: Option<String>,
    pub outputs: Vec<String>,
    /// A localised status key. The shell shows it.
    pub status: String,
    /// Optional text the shell shows with the key, such as a refusal reason.
    pub detail: Option<String>,
    pub failed: bool,
    /// A set was lost or timed out. Apply stays blocked until a reread.
    pub uncertain: bool,
    #[serde(skip)]
    job: Option<Job>,
    /// The config the draft started from. Compared at Apply to detect conflict.
    #[serde(skip)]
    base: Option<Value>,
    #[serde(skip)]
    conflict: bool,
    #[serde(skip)]
    pending: bool,
    #[serde(skip)]
    unavailable: bool,
    #[serde(skip)]
    refusal: bool,
    /// A state read was requested while a job ran; it runs once the job ends.
    #[serde(skip)]
    deferred: bool,
}

impl Default for WallpaperEditor {
    fn default() -> Self {
        Self {
            current: None,
            draft: None,
            target: None,
            outputs: Vec::new(),
            status: "status.loading".to_owned(),
            detail: None,
            failed: false,
            uncertain: false,
            job: None,
            base: None,
            conflict: false,
            pending: false,
            unavailable: false,
            refusal: false,
            deferred: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Job {
    ticket: u64,
    step: Step,
}

#[derive(Debug, Clone)]
enum Step {
    /// A plain state read.
    Read,
    /// The state read that gates an Apply.
    Preflight,
    /// The set, carrying the config it was sent with.
    Set(Value),
}

impl WallpaperEditor {
    pub fn can_edit(&self) -> bool { self.current.is_some() && !self.working() && !self.unavailable && !self.uncertain && !self.conflict }
    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    pub fn dirty(&self) -> bool {
        self.draft.is_some() && self.draft != self.current
    }

    pub fn can_apply(&self) -> bool {
        self.dirty()
            && !self.working()
            && !self.conflict
            && !self.uncertain
            && !self.unavailable
            && !self.pending
    }

    /// The fill the selected target names itself, or `Null` when it inherits.
    pub fn fill(&self) -> Value {
        let source = self.draft.clone().or_else(|| self.current.clone());
        source
            .and_then(|value| serde_json::from_value::<Document>(value).ok())
            .and_then(|document| document.fill(self.target.as_deref()))
            .map_or(Value::Null, |fill| {
                serde_json::to_value(fill).unwrap_or(Value::Null)
            })
    }

    /// Start a state read. During a job it is deferred into one read.
    pub fn read(&mut self, ticket: u64) -> Option<Call> {
        if self.working() {
            self.deferred = true;
            return None;
        }
        self.start(ticket, Step::Read, VERB_STATE, json!({}))
    }

    /// Start Apply with a preflight state read. The set follows only if the
    /// daemon is idle and the config has not moved since the draft began.
    pub fn apply(&mut self, ticket: u64) -> Option<Call> {
        if !self.can_apply() {
            return None;
        }
        self.status = "status.applying".to_owned();
        self.refusal = false;
        self.detail = None;
        self.start(ticket, Step::Preflight, VERB_STATE, json!({}))
    }

    /// Fold in the answer to `ticket`. Any other ticket is stale and ignored.
    /// `next_ticket` names the call this returns, if one is owed.
    pub fn complete(
        &mut self,
        ticket: u64,
        result: Result<Reply, CallError>,
        next_ticket: u64,
    ) -> Option<Call> {
        if self.job.as_ref().map(|job| job.ticket) != Some(ticket) {
            return None;
        }
        let step = self.job.take()?.step;
        match step {
            Step::Read => {
                self.read_state(result);
                self.reread(next_ticket, false)
            }
            Step::Preflight => self.preflight(result, next_ticket),
            Step::Set(requested) => self.finish_set(requested, result, next_ticket),
        }
    }

    /// Select the output to edit. `None` is the default. Unknown outputs are
    /// ignored.
    pub fn select_target(&mut self, target: Option<String>) {
        if let Some(name) = &target {
            if !self.outputs.contains(name) {
                return;
            }
        }
        self.target = target;
    }

    /// Theme removes the selected fill, so it inherits. Colour and image only
    /// record the kind; the value edit writes it.
    pub fn edit_kind(&mut self, kind: Kind) -> Result<(), &'static str> {
        match kind {
            Kind::Theme => self.edit(|document, target| {
                document.set_fill(target, None);
                Ok(())
            }),
            Kind::Colour | Kind::Image => Ok(()),
        }
    }

    pub fn edit_colour(&mut self, colour: String) -> Result<(), &'static str> {
        self.edit(move |document, target| {
            if !valid_colour(&colour) {
                return Err("status.invalid_colour");
            }
            document.set_fill(
                target,
                Some(Fill {
                    colour: Some(colour),
                    ..Fill::default()
                }),
            );
            Ok(())
        })
    }

    /// Replace the image path. The fallback colour is kept, as the file has it.
    pub fn edit_image(&mut self, path: String) -> Result<(), &'static str> {
        self.edit(move |document, target| {
            if !valid_path(&path) {
                return Err("status.invalid_image");
            }
            let fallback = document
                .fill(target)
                .filter(|fill| fill.image.is_some())
                .and_then(|fill| fill.fallback);
            document.set_fill(
                target,
                Some(Fill {
                    image: Some(path),
                    fallback,
                    ..Fill::default()
                }),
            );
            Ok(())
        })
    }

    /// Drop the draft and any conflict. Ignored during a job.
    pub fn revert(&mut self) {
        if self.working() {
            return;
        }
        self.draft = None;
        self.base = None;
        self.conflict = false;
        self.refusal = false;
        self.detail = None;
        self.settle();
    }

    fn start(&mut self, ticket: u64, step: Step, verb: &'static str, body: Value) -> Option<Call> {
        self.job = Some(Job { ticket, step });
        Some(Call { ticket, verb, body })
    }

    /// The one state read a finished job owes. A set always rereads; other
    /// jobs reread only when a read was deferred.
    fn reread(&mut self, ticket: u64, force: bool) -> Option<Call> {
        let deferred = std::mem::take(&mut self.deferred);
        if !force && !deferred {
            return None;
        }
        self.start(ticket, Step::Read, VERB_STATE, json!({}))
    }

    /// Take a state reply, or mark the state unavailable. Returns success.
    fn read_state(&mut self, result: Result<Reply, CallError>) -> bool {
        if let Ok(reply) = result && reply.rc == 0
            && let Ok(body) = serde_json::from_str::<Value>(&reply.body)
            && body["schema"].as_str() == Some(SCHEMA)
            && body.get("pending").is_some_and(|p| p.is_null() || p.as_u64().is_some())
            && let Some(config) = body.get("config")
            && parse(config.clone()).is_ok()
        {
            self.take_state(&body);
            return true;
        }
        self.unavailable = true;
        self.settle();
        false
    }

    fn take_state(&mut self, body: &Value) {
        self.current = body
            .get("config")
            .filter(|config| !config.is_null())
            .cloned();
        self.outputs = body
            .get("outputs")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|output| output.get("name")?.as_str().map(str::to_owned))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        if let Some(names) = self.current.as_ref().and_then(|c| c["outputs"].as_object()) {
            self.outputs.extend(names.keys().cloned());
        }
        self.outputs.sort(); self.outputs.dedup();
        self.pending = body.get("pending").is_some_and(|pending| !pending.is_null());
        self.unavailable = false;
        if !self.pending { self.uncertain = false; }
        // The daemon already holds the draft: nothing is left to apply.
        if self.draft == self.current {
            self.draft = None;
            self.base = None;
        }
        self.settle();
    }

    fn preflight(&mut self, result: Result<Reply, CallError>, next_ticket: u64) -> Option<Call> {
        if !self.read_state(result) {
            return self.reread(next_ticket, false);
        }
        if self.pending {
            self.settle();
            return self.reread(next_ticket, false);
        }
        if self.draft.is_none() { return self.reread(next_ticket, false); }
        if self.current != self.base {
            self.conflict = true;
            self.settle();
            return self.reread(next_ticket, false);
        }
        let Some(requested) = self.draft.clone() else {
            return self.reread(next_ticket, false);
        };
        self.status = "status.applying".to_owned();
        self.start(
            next_ticket,
            Step::Set(requested.clone()),
            VERB_SET,
            requested,
        )
    }

    fn finish_set(
        &mut self,
        requested: Value,
        result: Result<Reply, CallError>,
        next_ticket: u64,
    ) -> Option<Call> {
        match result {
            Ok(reply) if reply.rc == 0 => {
                let accepted = serde_json::from_str::<Value>(&reply.body).ok().is_some_and(|b| b["active"].as_u64().is_some() && b["requested"].as_u64().is_some());
                if !accepted {
                    self.uncertain = true;
                    self.settle();
                    return self.reread(next_ticket, true);
                }
                self.current = Some(requested);
                self.draft = None;
                self.base = None;
                self.uncertain = false;
                self.detail = None;
                self.settle();
                self.reread(next_ticket, true)
            }
            Ok(reply) => {
                // Refused: nothing changed, so current and draft stay as they were.
                self.detail = serde_json::from_str::<Value>(&reply.body).ok().as_ref()
                    .and_then(|b| b.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned).or(reply.error);
                self.refusal = true;
                self.status = "status.refused".to_owned();
                self.failed = true;
                self.reread(next_ticket, false)
            }
            Err(_) => {
                // Lost or timed out: it may have landed. Never resend; reread.
                self.uncertain = true;
                self.settle();
                self.reread(next_ticket, true)
            }
        }
    }

    /// Run one edit against the draft. The draft starts from `current`, or
    /// from a fresh rebase when a conflict is pending. Any failure sets status.
    fn edit(
        &mut self,
        change: impl FnOnce(&mut Document, Option<&str>) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let outcome = self.try_edit(change);
        if let Err(key) = outcome {
            self.status = key.to_owned();
            self.failed = true;
        }
        outcome
    }

    fn try_edit(
        &mut self,
        change: impl FnOnce(&mut Document, Option<&str>) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if self.current.is_none() || self.unavailable || self.uncertain { return Err("status.unavailable"); }
        if self.conflict { return Err("status.conflict"); }
        if self.working() {
            return Err("status.busy");
        }
        if let Some(name) = &self.target {
            if !self.outputs.contains(name) {
                return Err("status.unknown_output");
            }
        }
        let source = if self.conflict {
            self.current.clone()
        } else {
            self.draft.clone().or_else(|| self.current.clone())
        };
        let mut document = source.map_or(Ok(Document::blank()), parse)?;
        change(&mut document, self.target.as_deref())?;
        document.check()?;
        if self.draft.is_none() || self.conflict {
            self.base = self.current.clone();
        }
        self.conflict = false;
        self.refusal = false;
        self.detail = None;
        self.draft = Some(serde_json::to_value(&document).map_err(|_| "status.too_large")?);
        self.settle();
        Ok(())
    }

    /// Derive the status key and failure flag from the state. Refusals and
    /// applies set their own key, and it lasts until the next settle.
    fn settle(&mut self) {
        let key: &str = if self.unavailable {
            "status.unavailable"
        } else if self.uncertain {
            "status.uncertain"
        } else if self.conflict {
            "status.conflict"
        } else if self.pending {
            "status.pending"
        } else if self.refusal {
            "status.refused"
        } else if self.dirty() {
            "status.edited"
        } else {
            "status.ready"
        };
        self.status = key.to_owned();
        self.failed = self.unavailable || self.uncertain || self.conflict || self.refusal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(rc: u8, body: Value) -> Reply {
        Reply { rc, body:body.to_string(), error:None }
    }

    fn state(config: Value, pending: Value) -> Result<Reply, CallError> {
        Ok(reply(
            0,
            json!({
                "schema": SCHEMA,
                "config": config,
                "pending": pending,
                "outputs": [{"name": "DP-1"}, {"name": "DP-2"}],
            }),
        ))
    }

    fn lost() -> Result<Reply, CallError> {
        Err(CallError { message:"timed out".into(), outcome_unknown:true })
    }

    fn config() -> Value {
        json!({
            "schema": 1,
            "default": {"colour": "#112233"},
            "outputs": {
                "DP-1": {"image": "/a.png", "fallback": "#000000"},
                "DP-2": {"colour": "#ffffff"},
            },
        })
    }

    /// An editor that has read `config` once and is idle.
    fn loaded(config: Value) -> WallpaperEditor {
        let mut editor = WallpaperEditor::default();
        assert!(editor.read(1).is_some());
        assert!(editor.complete(1, state(config, Value::Null), 2).is_none());
        editor
    }

    /// Start Apply, answer its preflight with `latest`, and return what follows.
    fn preflight(
        editor: &mut WallpaperEditor,
        ticket: u64,
        latest: Result<Reply, CallError>,
    ) -> Option<Call> {
        let call = editor.apply(ticket).expect("preflight read");
        assert_eq!(call.verb, VERB_STATE);
        editor.complete(ticket, latest, ticket + 1)
    }

    #[test]
    fn edits_keep_other_overrides_and_fallback() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let draft = editor.draft.clone().unwrap();
        assert_eq!(draft["default"]["colour"], "#445566");
        assert_eq!(draft["outputs"], config()["outputs"]);

        editor.select_target(Some("DP-1".into()));
        editor.edit_image("/b.png".into()).unwrap();
        let draft = editor.draft.clone().unwrap();
        assert_eq!(
            draft["outputs"]["DP-1"],
            json!({"image": "/b.png", "fallback": "#000000"})
        );
        assert_eq!(draft["outputs"]["DP-2"], json!({"colour": "#ffffff"}));
        assert!(editor.can_apply());
    }

    #[test]
    fn theme_removes_only_the_selected_override() {
        let mut editor = loaded(config());
        editor.select_target(Some("DP-1".into()));
        editor.edit_kind(Kind::Theme).unwrap();
        let draft = editor.draft.clone().unwrap();
        assert!(draft["outputs"].get("DP-1").is_none());
        assert_eq!(draft["outputs"]["DP-2"], json!({"colour": "#ffffff"}));
        assert_eq!(draft["default"], json!({"colour": "#112233"}));

        editor.select_target(None);
        editor.edit_kind(Kind::Theme).unwrap();
        let draft = editor.draft.clone().unwrap();
        assert!(draft.get("default").is_none());
    }

    #[test]
    fn invalid_values_are_refused() {
        let mut editor = loaded(config());
        assert_eq!(editor.edit_colour("#12345".into()), Err("status.invalid_colour"));
        assert_eq!(editor.edit_image("relative.png".into()), Err("status.invalid_image"));
        assert_eq!(editor.edit_image("/a\0b.png".into()), Err("status.invalid_image"));
        assert!(editor.draft.is_none());
    }

    #[test]
    fn unknown_config_refuses_editing() {
        let mut editor = loaded(json!({"schema": 1, "extra": true}));
        assert!(!editor.can_edit());
        assert!(!editor.can_apply());
        assert_eq!(editor.status, "status.unavailable");
    }

    #[test]
    fn stale_reply_is_ignored() {
        let mut editor = WallpaperEditor::default();
        editor.read(1).unwrap();
        assert!(editor.complete(7, state(config(), Value::Null), 2).is_none());
        assert!(editor.working());
        assert!(editor.current.is_none());
    }

    #[test]
    fn reads_during_a_job_coalesce_into_one() {
        let mut editor = WallpaperEditor::default();
        editor.read(1).unwrap();
        assert!(editor.read(2).is_none());
        assert!(editor.read(3).is_none());
        let call = editor
            .complete(1, state(config(), Value::Null), 4)
            .expect("one deferred read");
        assert_eq!((call.ticket, call.verb), (4, VERB_STATE));
        assert!(editor.complete(4, state(config(), Value::Null), 5).is_none());
        assert!(!editor.working());
    }

    #[test]
    fn pending_set_refuses_apply() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        assert!(preflight(&mut editor, 10, state(config(), json!(7))).is_none());
        assert_eq!(editor.status, "status.pending");
        assert!(!editor.can_apply());
        assert!(editor.draft.is_some());
    }

    #[test]
    fn conflict_keeps_draft_until_re_edit() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let moved = json!({"schema": 1, "default": {"colour": "#010101"}});
        assert!(preflight(&mut editor, 10, state(moved, Value::Null)).is_none());
        assert_eq!(editor.status, "status.conflict");
        assert!(editor.failed);
        assert!(!editor.can_apply());
        assert_eq!(editor.draft.as_ref().unwrap()["default"]["colour"], "#445566");

        assert_eq!(editor.edit_colour("#445566".into()), Err("status.conflict"));
        editor.revert();
        editor.edit_colour("#445566".into()).unwrap();
        assert!(editor.can_apply());
        assert!(editor.apply(11).is_some());
    }

    #[test]
    fn refused_set_keeps_current_and_draft() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let draft = editor.draft.clone();
        let call = preflight(&mut editor, 10, state(config(), Value::Null)).expect("set");
        assert_eq!(call.verb, VERB_SET);
        assert!(editor
            .complete(call.ticket, Ok(reply(10, json!({"message": "no such output"}))), 12)
            .is_none());
        assert_eq!(editor.current, Some(config()));
        assert_eq!(editor.draft, draft);
        assert_eq!(editor.status, "status.refused");
        assert_eq!(editor.detail.as_deref(), Some("no such output"));
        assert!(editor.failed);
    }

    #[test]
    fn accepted_set_commits_draft_and_rereads() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let call = preflight(&mut editor, 10, state(config(), Value::Null)).expect("set");
        let reread = editor
            .complete(call.ticket, Ok(reply(0, json!({"requested": 1, "active": 1}))), 12)
            .expect("reread");
        assert_eq!(reread.verb, VERB_STATE);
        assert!(editor.draft.is_none());
        assert_eq!(editor.current.as_ref().unwrap()["default"]["colour"], "#445566");
    }

    #[test]
    fn lost_set_blocks_apply_until_reread_and_never_resends() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let landed = editor.draft.clone().unwrap();
        let call = preflight(&mut editor, 10, state(config(), Value::Null)).expect("set");
        let reread = editor.complete(call.ticket, lost(), 12).expect("reread");
        assert_eq!(reread.verb, VERB_STATE);
        assert!(editor.uncertain);
        assert_eq!(editor.status, "status.uncertain");
        assert!(!editor.can_apply());
        assert!(editor.apply(13).is_none());

        // A successful reread that shows the set landed clears the uncertainty.
        assert!(editor
            .complete(12, state(landed, Value::Null), 14)
            .is_none());
        assert!(!editor.uncertain);
        assert!(editor.draft.is_none());
        assert!(!editor.can_apply());
    }

    #[test]
    fn failed_state_read_shows_unavailable() {
        let mut editor = WallpaperEditor::default();
        editor.read(1).unwrap();
        assert!(editor.complete(1, lost(), 2).is_none());
        assert_eq!(editor.status, "status.unavailable");
        assert!(editor.failed);
        assert!(!editor.can_apply());
    }

    #[test]
    fn an_already_applied_draft_does_not_conflict_or_resend() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let landed = editor.draft.clone().unwrap();
        assert!(preflight(&mut editor, 10, state(landed, Value::Null)).is_none());
        assert!(editor.draft.is_none());
        assert_eq!(editor.status, "status.ready");
    }

    #[test]
    fn uncertainty_stays_while_a_set_is_pending() {
        let mut editor = loaded(config());
        editor.edit_colour("#445566".into()).unwrap();
        let call = preflight(&mut editor, 10, state(config(), Value::Null)).unwrap();
        editor.complete(call.ticket, lost(), 12).unwrap();
        editor.complete(12, state(config(), json!(3)), 13);
        assert!(editor.uncertain);
        assert!(!editor.can_apply());
    }
}
