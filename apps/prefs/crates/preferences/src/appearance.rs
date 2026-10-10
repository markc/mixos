// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Appearance panel over settingsd (docs/spec/settings/): the session's
//! colour scheme, style, mode, contrast and window framing.
//!
//! The panel reads the desktop profile (`settings.get`), keeps the person's
//! edits as a draft beside it, and writes the draft in one fenced
//! `settings.apply`. Another writer's change arrives as a hint on the
//! profile's topic and is re-read over the authenticated call; a draft
//! survives it: the draft is the axes the person edited, and it follows
//! settingsd on every other axis. An edited axis another writer changed to
//! something else is a conflict, applied over only once the person keeps
//! their edits. Every apply is fenced on the revision shown and sends only
//! the edited axes, so it never lands on an unseen change, nor twice. An
//! apply whose reply is lost is settled by its receipt (`settings.status`
//! with the operation id) before anything else is applied; when settingsd
//! holds no receipt (evicted, or never accepted) the next read decides what
//! is reported, never a guess. Quitting waits for a bounded number of
//! receipt lookups; nothing needs to survive a restart, since the fence and
//! a fresh read already make any later apply safe.
use crate::engine::{Effect, Engine};
use citizen::{CallError, Reply};
use design::{CaptionSide, Contrast, Decorations, DesignContext, Mode, Scheme, Style};
use serde::Serialize;
use serde_json::{Value, json};
use settings::model::{Appearance as Stored, ApplyRequest, Binding, Revision};
use std::collections::BTreeMap;

/// The Bus service behind the Appearance panel.
pub const SETTINGS: &str = "settingsd";
/// The desktop profile the panel edits.
pub const PROFILE: &str = "default";

/// The topic settingsd announces the profile's changes on.
pub fn topic() -> String {
    settings::topic(PROFILE)
}

/// One appearance, as the panel shows and edits it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub scheme: Scheme,
    /// `None` is the scheme's own style.
    pub style: Option<Style>,
    pub mode: Mode,
    pub contrast: Contrast,
    pub decorations: Decorations,
    pub captions: CaptionSide,
}

fn named<T>(what: &str, name: &str, from_name: fn(&str) -> Option<T>) -> Result<T, String> {
    from_name(name).ok_or_else(|| format!("unknown {what} {name:?}"))
}

impl Look {
    /// The profile's stored appearance.
    pub fn from_stored(stored: &Stored) -> Result<Self, String> {
        Ok(Self {
            scheme: named("scheme", &stored.scheme, Scheme::from_name)?,
            style: match &stored.style {
                None => None,
                Some(name) => Some(named("style", name, Style::from_name)?),
            },
            mode: named("mode", &stored.mode, Mode::from_name)?,
            contrast: named("contrast", &stored.contrast, Contrast::from_name)?,
            decorations: named("decorations", &stored.decorations, Decorations::from_name)?,
            captions: named("caption side", &stored.caption_side, CaptionSide::from_name)?,
        })
    }

    /// The design context this look draws with.
    pub fn context(&self) -> DesignContext {
        DesignContext {
            scheme: self.scheme,
            mode: self.mode,
            contrast: self.contrast,
            style: self.style,
            ..DesignContext::default()
        }
    }

    /// The `settings.apply` changes that turn `base` into this look.
    pub fn changes(&self, base: &Look) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        let mut put = |key: &str, differs: bool, value: Value| {
            if differs {
                out.insert(format!("appearance.{key}"), value);
            }
        };
        put(
            "scheme",
            self.scheme != base.scheme,
            json!(self.scheme.name()),
        );
        // null is the scheme's own style.
        put(
            "style",
            self.style != base.style,
            json!(self.style.map(Style::name)),
        );
        put("mode", self.mode != base.mode, json!(self.mode.name()));
        put(
            "contrast",
            self.contrast != base.contrast,
            json!(self.contrast.name()),
        );
        put(
            "decorations",
            self.decorations != base.decorations,
            json!(self.decorations.name()),
        );
        put(
            "caption_side",
            self.captions != base.captions,
            json!(self.captions.name()),
        );
        out
    }

    /// This look with the axes `args` names: `scheme`, `style` (a style or
    /// "own"), `mode`, `contrast`, `decorations`, `caption_side`. An absent
    /// key leaves its axis; every present one is checked before any applies.
    pub fn with(&self, args: &Value) -> Result<Self, String> {
        fn axis<T>(
            args: &Value,
            key: &str,
            from_name: fn(&str) -> Option<T>,
        ) -> Result<Option<T>, String> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::String(name)) => named(key, name, from_name).map(Some),
                Some(_) => Err(format!("{key} must be a name")),
            }
        }
        fn style(name: &str) -> Option<Option<Style>> {
            if name == "own" {
                Some(None)
            } else {
                Style::from_name(name).map(Some)
            }
        }
        let mut look = *self;
        if let Some(scheme) = axis(args, "scheme", Scheme::from_name)? {
            look.scheme = scheme;
        }
        if let Some(style) = axis(args, "style", style)? {
            look.style = style;
        }
        if let Some(mode) = axis(args, "mode", Mode::from_name)? {
            look.mode = mode;
        }
        if let Some(contrast) = axis(args, "contrast", Contrast::from_name)? {
            look.contrast = contrast;
        }
        if let Some(decorations) = axis(args, "decorations", Decorations::from_name)? {
            look.decorations = decorations;
        }
        if let Some(captions) = axis(args, "caption_side", CaptionSide::from_name)? {
            look.captions = captions;
        }
        Ok(look)
    }
}

impl Serialize for Look {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({"scheme":self.scheme.name(),"style":self.style.map_or("own", Style::name),
            "mode":self.mode.name(),"contrast":self.contrast.name(),
            "decorations":self.decorations.name(),"caption_side":self.captions.name()})
        .serialize(s)
    }
}

/// settingsd as the panel last saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unknown,
    Available,
    /// Not registered on the Bus: the panel says how to start it.
    Missing,
}

/// One axis of a [`Look`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Scheme,
    Style,
    Mode,
    Contrast,
    Decorations,
    CaptionSide,
}

impl Axis {
    pub const ALL: [Axis; 6] = [
        Axis::Scheme,
        Axis::Style,
        Axis::Mode,
        Axis::Contrast,
        Axis::Decorations,
        Axis::CaptionSide,
    ];

    /// `a` and `b` differ on this axis.
    pub fn differs(self, a: &Look, b: &Look) -> bool {
        match self {
            Axis::Scheme => a.scheme != b.scheme,
            Axis::Style => a.style != b.style,
            Axis::Mode => a.mode != b.mode,
            Axis::Contrast => a.contrast != b.contrast,
            Axis::Decorations => a.decorations != b.decorations,
            Axis::CaptionSide => a.captions != b.captions,
        }
    }

    /// Copy this axis from `from` into `to`.
    fn copy(self, to: &mut Look, from: &Look) {
        match self {
            Axis::Scheme => to.scheme = from.scheme,
            Axis::Style => to.style = from.style,
            Axis::Mode => to.mode = from.mode,
            Axis::Contrast => to.contrast = from.contrast,
            Axis::Decorations => to.decorations = from.decorations,
            Axis::CaptionSide => to.captions = from.captions,
        }
    }
}

/// The person's edits: the look they were made on and the look they leave.
/// An axis is edited when the two differ there; every other axis follows
/// settingsd.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Draft {
    pub base: Look,
    pub look: Look,
}

impl Draft {
    pub fn edits(&self) -> Vec<Axis> {
        Axis::ALL
            .into_iter()
            .filter(|a| a.differs(&self.base, &self.look))
            .collect()
    }
}

/// A submitted apply: its operation id and what it asked for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Submitted {
    pub operation: String,
    pub look: Look,
    pub axes: Vec<Axis>,
}

impl Submitted {
    /// `current` shows every axis this apply asked for.
    fn shown_in(&self, current: &Look) -> bool {
        self.axes.iter().all(|a| !a.differs(&self.look, current))
    }
}

/// One call to settingsd.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    Read,
    Apply(Submitted),
    /// Settle an apply whose reply was lost, by its receipt.
    Receipt(Submitted),
}

impl Call {
    fn verb(&self) -> &'static str {
        match self {
            Call::Read => "settings.get",
            Call::Apply(_) => "settings.apply",
            Call::Receipt(_) => "settings.status",
        }
    }
}

/// How many receipt lookups quitting waits for before it closes anyway.
const QUIT_RECEIPT_TRIES: u32 = 3;

/// The Appearance panel's state.
#[derive(Clone, Debug)]
pub struct Appearance {
    /// The profile edited ([`PROFILE`]); its instance is adopted from
    /// settingsd's answer when the first guess (the host name) is not the
    /// one it serves. The profile is never adopted.
    pub binding: Binding,
    retargeted: bool,
    pub settings: Availability,
    /// The look settingsd holds.
    pub current: Option<Look>,
    /// The profile names a design package of its own (`appearance.source`):
    /// the window keeps the session theme rather than the embedded design.
    pub custom_source: bool,
    /// The fence for the next apply: settingsd's incarnation and revision.
    fence: Option<(String, Revision)>,
    /// The person's edits, until applied or reverted.
    pub draft: Option<Draft>,
    /// Edited axes another writer changed too, to something else: nothing
    /// is applied until the person keeps their edits or reverts.
    pub conflicts: Vec<Axis>,
    /// An apply whose outcome is unknown: settled by its receipt before
    /// anything else is applied.
    pub uncertain: Option<Submitted>,
    /// An apply settingsd holds no receipt for (evicted, or never
    /// accepted): the next read tells whether the session shows it.
    confirm: Option<Submitted>,
    /// An apply succeeded and the read that shows its result has not: the
    /// panel's look is stale until it does.
    syncing: bool,
    pub(crate) job: Option<(u64, Call)>,
    reread: bool,
    applies: u64,
    quit_tries: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            binding: Binding {
                instance: "host".into(),
                profile: PROFILE.into(),
            },
            retargeted: false,
            settings: Availability::Unknown,
            current: None,
            custom_source: false,
            fence: None,
            draft: None,
            conflicts: Vec::new(),
            uncertain: None,
            confirm: None,
            syncing: false,
            job: None,
            reread: false,
            applies: 0,
            quit_tries: 0,
        }
    }
}

impl Appearance {
    /// The look the panel and the window show: the draft, else settingsd's.
    pub fn shown(&self) -> Option<Look> {
        self.draft.map(|d| d.look).or(self.current)
    }

    /// A settingsd call is in flight.
    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    /// An apply is in flight: the controls wait for its answer.
    pub fn applying(&self) -> bool {
        matches!(self.job, Some((_, Call::Apply(_))))
    }

    /// The revision the panel was read at.
    pub fn revision(&self) -> Option<u64> {
        self.fence.as_ref().map(|(_, r)| r.0)
    }

    /// settingsd's new look: the draft follows it on every axis the person
    /// did not edit. An edited axis another writer changed to something
    /// else is a conflict; one changed to the person's value is no longer
    /// an edit.
    fn rebase(&mut self, current: Look) {
        let Some(draft) = self.draft else {
            return;
        };
        for axis in draft.edits() {
            if axis.differs(&current, &draft.base)
                && axis.differs(&current, &draft.look)
                && !self.conflicts.contains(&axis)
            {
                self.conflicts.push(axis);
            }
        }
        let mut look = current;
        for axis in draft.edits() {
            axis.copy(&mut look, &draft.look);
        }
        self.draft = (look != current).then_some(Draft {
            base: current,
            look,
        });
        self.conflicts
            .retain(|a| self.draft.is_some_and(|d| a.differs(&d.base, &d.look)));
    }

    /// The submitted axes are settingsd's now: they stop being edits.
    fn retire(&mut self, submitted: &Submitted) {
        let Some(mut draft) = self.draft else {
            return;
        };
        for axis in &submitted.axes {
            if !axis.differs(&draft.look, &submitted.look) {
                axis.copy(&mut draft.look, &draft.base);
                axis.copy(&mut draft.base, &submitted.look);
                axis.copy(&mut draft.look, &submitted.look);
            }
        }
        self.draft = (draft.base != draft.look).then_some(draft);
        self.conflicts.retain(|a| !submitted.axes.contains(a));
    }
}

/// What a settingsd call answered.
enum Answer {
    /// rc 0 with a JSON body.
    Ok(Value),
    /// settingsd refused (rc 10, a JSON body with a `status`).
    Refused(Value),
    /// Not registered on the Bus.
    Missing,
    /// Refused before reaching settingsd: nothing happened.
    NotSent(String),
    /// The reply was lost after the call may have arrived.
    Lost(String),
}

fn answer(result: Result<Reply, CallError>) -> Answer {
    let reply = match result {
        Ok(reply) => reply,
        Err(error) if error.outcome_unknown => return Answer::Lost(error.message),
        Err(error) => return Answer::NotSent(error.message),
    };
    if reply.body.trim().is_empty() {
        let not_found = reply
            .error
            .as_deref()
            .is_none_or(|e| e.contains("not found"));
        if reply.rc == 10 && not_found {
            return Answer::Missing;
        }
        return Answer::NotSent(
            reply
                .error
                .unwrap_or_else(|| format!("refused by the broker (rc {})", reply.rc)),
        );
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::String(reply.body));
    if reply.rc == 0 {
        Answer::Ok(value)
    } else {
        Answer::Refused(value)
    }
}

/// A refusal's text: its message, else its diagnostics, else its status.
fn refusal(value: &Value) -> String {
    if let Some(message) = value["message"].as_str() {
        return message.to_owned();
    }
    if let Some(list) = value["diagnostics"].as_array() {
        let lines: Vec<String> = list
            .iter()
            .map(|d| {
                format!(
                    "{}: {}",
                    d["path"].as_str().unwrap_or_default(),
                    d["message"].as_str().unwrap_or_default()
                )
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("; ");
        }
    }
    value["status"]
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// The fence and look in a `settings.get` reply.
fn snapshot(value: &Value) -> Result<(String, Revision, Look, bool), String> {
    let snapshot = &value["snapshot"];
    let incarnation = snapshot["incarnation"]
        .as_str()
        .ok_or("the snapshot has no incarnation")?
        .to_owned();
    let revision: Revision = serde_json::from_value(snapshot["revision"].clone())
        .map_err(|e| format!("revision: {e}"))?;
    let stored: Stored = serde_json::from_value(snapshot["desktop"]["appearance"].clone())
        .map_err(|e| format!("appearance: {e}"))?;
    let look = Look::from_stored(&stored)?;
    Ok((incarnation, revision, look, stored.source.is_some()))
}

impl Engine {
    /// Start the panel on the profile of settingsd `instance` (the host
    /// name; adopted from settingsd's answer when it serves another).
    pub fn start_appearance(&mut self, instance: &str) {
        let binding = Binding {
            instance: instance.to_owned(),
            profile: PROFILE.into(),
        };
        if binding.validate().is_ok() {
            self.look.binding = binding;
        }
        self.read_look();
    }

    /// Read the profile again (a hint arrived, the Bus reconnected, Refresh),
    /// or, with an apply still uncertain, ask for its receipt first. While
    /// quitting only a receipt is asked for, a bounded number of times.
    pub fn read_look(&mut self) {
        if self.look.job.is_some() {
            self.look.reread = true;
            return;
        }
        if !self.connected {
            return;
        }
        match self.look.uncertain.clone() {
            Some(submitted) => {
                if self.quitting {
                    if self.look.quit_tries >= QUIT_RECEIPT_TRIES {
                        return;
                    }
                    self.look.quit_tries += 1;
                }
                self.call_settings(Call::Receipt(submitted));
            }
            None if !self.quitting => self.call_settings(Call::Read),
            None => {}
        }
    }

    fn call_settings(&mut self, call: Call) {
        let ticket = self.next();
        let body = match &call {
            Call::Read => json!({"binding":self.look.binding}),
            Call::Receipt(submitted) => {
                json!({"binding":self.look.binding,"operation_id":submitted.operation})
            }
            Call::Apply(submitted) => {
                let (incarnation, revision) = self.look.fence.clone().expect("a fenced apply");
                let current = self.look.current.unwrap_or_default();
                let changes = submitted
                    .look
                    .changes(&current)
                    .into_iter()
                    .filter(|(key, _)| {
                        submitted
                            .axes
                            .iter()
                            .any(|a| key == &format!("appearance.{}", axis_key(*a)))
                    })
                    .collect();
                serde_json::to_value(ApplyRequest {
                    binding: self.look.binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: submitted.operation.clone(),
                    changes,
                    reset: Vec::new(),
                    request_digest: None,
                })
                .expect("an apply request serialises")
            }
        };
        let verb = call.verb();
        self.look.job = Some((ticket, call));
        self.effects.push(Effect::Settings { ticket, verb, body });
    }

    /// The panel takes edits: read, not applying, no dialog, not quitting.
    /// No edits while an apply is in flight, uncertain, or applied but not
    /// yet read back: an edit measured against a base that apply changed
    /// could be lost or misread. A failed read-back keeps them waiting
    /// (Refresh reads again).
    pub fn can_edit_look(&self) -> bool {
        self.look.current.is_some()
            && !self.look.applying()
            && self.look.uncertain.is_none()
            && self.look.confirm.is_none()
            && !self.look.syncing
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    /// Edit the draft: the panel's controls and `prefs.appearance.set`.
    /// `look` is the whole look the person now wants; it is measured
    /// against settingsd's, so a look equal to it is no draft.
    pub fn edit_look(&mut self, look: Look) {
        if !self.can_edit_look() {
            return;
        }
        let Some(current) = self.look.current else {
            return;
        };
        let base = self.look.draft.map_or(current, |d| d.base);
        self.look.draft = (look != base).then_some(Draft { base, look });
        let draft = self.look.draft;
        self.look
            .conflicts
            .retain(|a| draft.is_some_and(|d| a.differs(&d.base, &d.look)));
    }

    pub fn can_apply_look(&self) -> bool {
        self.look.draft.is_some()
            && self.look.conflicts.is_empty()
            && self.look.fence.is_some()
            && self.look.job.is_none()
            && self.look.uncertain.is_none()
            && self.look.settings == Availability::Available
            && self.connected
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    pub fn can_revert_look(&self) -> bool {
        self.look.draft.is_some() && self.can_edit_look()
    }

    /// The edits stand over another writer's change: the conflict is
    /// acknowledged and Apply is allowed again.
    pub fn can_keep_look(&self) -> bool {
        !self.look.conflicts.is_empty() && self.can_edit_look()
    }

    /// An uncertain apply can be checked again now.
    pub fn can_recheck_look(&self) -> bool {
        self.look.uncertain.is_some() && self.look.job.is_none() && self.connected
    }

    /// Write the edited axes to settingsd, fenced on the revision read.
    pub fn apply_look(&mut self) {
        if !self.can_apply_look() {
            return;
        }
        let Some(draft) = self.look.draft else {
            return;
        };
        self.look.applies += 1;
        let operation = format!(
            "prefs-{}-{}-{}",
            std::process::id(),
            self.look.applies,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        );
        self.status = self.label("applying-look", &[]);
        self.failed = false;
        self.call_settings(Call::Apply(Submitted {
            operation,
            look: draft.look,
            axes: draft.edits(),
        }));
    }

    /// Drop the draft: the window shows settingsd's look again.
    pub fn revert_look(&mut self) {
        if !self.can_revert_look() {
            return;
        }
        self.look.draft = None;
        self.look.conflicts.clear();
        self.status = self.label("reverted-look", &[]);
        self.failed = false;
    }

    /// Keep the edits over the other writer's change (then Apply).
    pub fn keep_look(&mut self) {
        if self.can_keep_look() {
            self.look.conflicts.clear();
            self.status = self.label("kept-look", &[]);
            self.failed = false;
        }
    }

    /// Ask again for an uncertain apply's receipt.
    pub fn recheck_look(&mut self) {
        if self.can_recheck_look() {
            self.read_look();
        }
    }

    /// settingsd call `ticket` finished.
    pub fn settled(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        let Some((expected, call)) = self.look.job.clone() else {
            return;
        };
        if expected != ticket {
            return;
        }
        self.look.job = None;
        let answer = answer(result);
        if let Answer::Missing = answer {
            self.look.settings = Availability::Missing;
            if !matches!(call, Call::Read) {
                self.status = self.label("settings-missing", &[]);
                self.failed = true;
            }
        } else {
            match call {
                Call::Read => self.read_answered(answer),
                Call::Apply(submitted) => self.apply_answered(submitted, answer),
                Call::Receipt(submitted) => self.receipt_answered(submitted, answer),
            }
        }
        if self.look.reread && self.look.job.is_none() {
            self.look.reread = false;
            self.read_look();
        }
        if self.quitting && !self.busy() {
            self.effects.push(Effect::Exit);
        }
    }

    fn read_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(value) => match snapshot(&value) {
                Ok((incarnation, revision, look, custom)) => {
                    self.look.settings = Availability::Available;
                    self.look.fence = Some((incarnation, revision));
                    self.look.current = Some(look);
                    self.look.custom_source = custom;
                    self.look.syncing = false;
                    self.look.rebase(look);
                    if let Some(submitted) = self.look.confirm.take() {
                        if submitted.shown_in(&look) {
                            self.look.retire(&submitted);
                            self.status = self.label("applied-look", &[]);
                            self.failed = false;
                        } else {
                            self.status = self.label("look-unconfirmed", &[]);
                            self.failed = true;
                        }
                    }
                }
                Err(message) => self.look_failed(&message),
            },
            Answer::Refused(value)
                if value["status"] == "wrong_target" && !self.look.retargeted =>
            {
                // Only the instance is adopted: another profile is never
                // edited (nor its topic followed) by accident.
                match serde_json::from_value::<Binding>(value["binding"].clone()) {
                    Ok(binding) if binding.validate().is_ok() && binding.profile == PROFILE => {
                        self.look.binding.instance = binding.instance;
                        self.look.retargeted = true;
                        self.call_settings(Call::Read);
                    }
                    _ => self.look_failed(&refusal(&value)),
                }
            }
            Answer::Refused(value) => self.look_failed(&refusal(&value)),
            Answer::NotSent(message) | Answer::Lost(message) => self.look_failed(&message),
            Answer::Missing => {}
        }
    }

    fn apply_answered(&mut self, submitted: Submitted, answer: Answer) {
        match answer {
            Answer::Ok(_) => {
                self.look.retire(&submitted);
                self.look.syncing = true;
                self.status = self.label("applied-look", &[]);
                self.failed = false;
                self.look.reread = true;
            }
            Answer::Refused(value) if value["status"] == "conflict" => {
                self.status = self.label("look-conflict", &[]);
                self.failed = true;
                self.look.reread = true;
            }
            Answer::Refused(value) if value["status"] == "outcome_unknown" => {
                self.uncertain_apply(submitted, &refusal(&value));
            }
            Answer::Lost(message) => self.uncertain_apply(submitted, &message),
            Answer::Refused(value) => self.look_failed(&refusal(&value)),
            Answer::NotSent(message) => self.look_failed(&message),
            Answer::Missing => {}
        }
    }

    fn uncertain_apply(&mut self, submitted: Submitted, message: &str) {
        self.look.uncertain = Some(submitted);
        self.status = self.label("look-uncertain", &[("message", message)]);
        self.failed = true;
        // Ask for the receipt next (read_look does, while uncertain).
        self.look.reread = true;
    }

    fn receipt_answered(&mut self, submitted: Submitted, answer: Answer) {
        let operation = submitted.operation.clone();
        match answer {
            Answer::Ok(value)
                if value["status"] == "current"
                    && value["receipt"]["operation_id"] == operation.as_str() =>
            {
                self.look.uncertain = None;
                self.look.retire(&submitted);
                self.look.syncing = true;
                self.status = self.label("applied-look", &[]);
                self.failed = false;
                self.look.reread = true;
            }
            // No receipt, and settingsd is not recovering: it may have been
            // evicted or never accepted. Not a verdict: the next read says
            // whether the session shows the change.
            Answer::Ok(value)
                if value["status"] == "unknown_operation" && value["recovering"] != true =>
            {
                self.look.uncertain = None;
                self.look.confirm = Some(submitted);
                self.look.reread = true;
            }
            // Still unknown (settingsd recovering, or the lookup failed):
            // Check again, Refresh, a hint or a reconnection asks again.
            Answer::Ok(value) | Answer::Refused(value) => self.receipt_pending(&refusal(&value)),
            Answer::NotSent(message) | Answer::Lost(message) => self.receipt_pending(&message),
            Answer::Missing => {}
        }
    }

    fn receipt_pending(&mut self, message: &str) {
        self.status = self.label("look-uncertain", &[("message", message)]);
        self.failed = true;
        if self.quitting {
            self.look.reread = true;
        }
    }

    fn look_failed(&mut self, message: &str) {
        self.status = self.label("look-failed", &[("message", message)]);
        self.failed = true;
    }

    /// The panel as `prefs.appearance` answers it.
    pub fn appearance_view(&self) -> Value {
        json!({"settings":self.look.settings,"binding":self.look.binding,
            "revision":self.look.revision().map(|r| r.to_string()),
            "current":self.look.current,"draft":self.look.draft.map(|d| d.look),
            "edits":self.look.draft.map(|d| d.edits()).unwrap_or_default(),
            "conflicts":self.look.conflicts,"shown":self.look.shown(),
            "custom_source":self.look.custom_source,
            "uncertain":self.look.uncertain.as_ref().map(|s| &s.operation),
            "working":self.look.job.as_ref().map(|(_, c)| c.verb()),
            "can_apply":self.can_apply_look(),"can_revert":self.can_revert_look(),
            "can_keep":self.can_keep_look(),"can_recheck":self.can_recheck_look()})
    }

    /// `prefs.appearance.set`: edit the draft from what is shown.
    pub(crate) fn set_look(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        let Some(base) = self.look.shown() else {
            return Err(("NOT_READY", self.label("look-not-ready", &[])));
        };
        if !self.can_edit_look() {
            return Err(("BUSY", self.label("busy", &[])));
        }
        let look = base.with(args).map_err(|e| ("ARGUMENT", e))?;
        self.edit_look(look);
        Ok(self.appearance_view())
    }
}

/// An axis's `settings.apply` key, after `appearance.`.
fn axis_key(axis: Axis) -> &'static str {
    match axis {
        Axis::Scheme => "scheme",
        Axis::Style => "style",
        Axis::Mode => "mode",
        Axis::Contrast => "contrast",
        Axis::Decorations => "decorations",
        Axis::CaptionSide => "caption_side",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Effect;

    fn label(key: &str, args: &[(&str, &str)]) -> String {
        let mut out = key.to_owned();
        for (k, v) in args {
            out.push_str(&format!(" {k}={v}"));
        }
        out
    }

    fn reply(rc: u8, body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc,
            body: body.to_string(),
            error: None,
        })
    }

    fn lost() -> Result<Reply, CallError> {
        Err(CallError {
            message: "timed out".into(),
            outcome_unknown: true,
        })
    }

    fn snapshot_with(revision: &str, appearance: Value) -> Value {
        json!({"status":"current","snapshot":{"incarnation":"inc-1","revision":revision,
            "desktop":{"appearance":appearance}},"publication_pending":false,"recovering":false})
    }

    fn snapshot_body(revision: &str, scheme: &str, mode: &str) -> Value {
        snapshot_with(
            revision,
            json!({"scheme":scheme,"mode":mode,"contrast":"normal","source":null}),
        )
    }

    fn settings_calls(e: &mut Engine) -> Vec<(u64, &'static str, Value)> {
        e.take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Settings { ticket, verb, body } => Some((ticket, verb, body)),
                _ => None,
            })
            .collect()
    }

    /// An engine whose panel has read revision 7 (studio, the scheme's own
    /// style, dark).
    fn read() -> Engine {
        let mut e = Engine::new(label);
        // The Applications panel's first listing, answered so nothing else
        // is in flight.
        let listing = e
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Releases { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("the first listing");
        e.released(listing, reply(0, json!([])));
        e.take_effects();
        e.start_appearance("example");
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "settings.get");
        assert_eq!(calls[0].2["binding"]["instance"], "example");
        e.settled(calls[0].0, reply(0, snapshot_body("7", "studio", "dark")));
        e
    }

    fn edit(e: &mut Engine, args: Value) {
        let look = e.look.shown().unwrap().with(&args).unwrap();
        e.edit_look(look);
    }

    /// Answer the next settings call with `body`.
    fn answer_next(e: &mut Engine, rc: u8, body: Value) -> (&'static str, Value) {
        let calls = settings_calls(e);
        let (ticket, verb, sent) = calls[0].clone();
        e.settled(ticket, reply(rc, body));
        (verb, sent)
    }

    #[test]
    fn the_first_read_fills_the_panel() {
        let e = read();
        assert_eq!(e.look.settings, Availability::Available);
        assert_eq!(e.look.current.unwrap().scheme, Scheme::Studio);
        assert_eq!(e.look.revision(), Some(7));
        assert!(e.look.draft.is_none() && !e.can_apply_look());
    }

    #[test]
    fn a_wrong_target_adopts_the_instance_once_never_the_profile() {
        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("guess");
        answer_next(
            &mut e,
            10,
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"default"}}),
        );
        let second = settings_calls(&mut e);
        assert_eq!(second[0].2["binding"]["instance"], "real");
        e.settled(
            second[0].0,
            reply(
                10,
                json!({"status":"wrong_target","binding":{"instance":"other","profile":"default"}}),
            ),
        );
        assert!(
            settings_calls(&mut e).is_empty(),
            "a second retarget is refused"
        );
        assert!(e.failed);

        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("guess");
        answer_next(
            &mut e,
            10,
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"other"}}),
        );
        assert!(
            settings_calls(&mut e).is_empty(),
            "another profile is never adopted"
        );
        assert_eq!(e.look.binding.profile, PROFILE);
    }

    #[test]
    fn an_edit_back_to_the_current_look_is_no_draft() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        assert!(e.can_apply_look() && e.can_revert_look());
        edit(&mut e, json!({"scheme":"studio"}));
        assert!(e.look.draft.is_none());
    }

    #[test]
    fn apply_sends_only_the_edited_axes_fenced_and_retires_them() {
        let mut e = read();
        edit(
            &mut e,
            json!({"scheme":"forest","style":"pro","decorations":"ssd"}),
        );
        e.apply_look();
        let calls = settings_calls(&mut e);
        let (ticket, verb, body) = calls[0].clone();
        assert_eq!(verb, "settings.apply");
        assert_eq!(body["expected_incarnation"], "inc-1");
        assert_eq!(body["expected_revision"], "7");
        assert_eq!(
            body["changes"],
            json!({"appearance.scheme":"forest","appearance.style":"pro","appearance.decorations":"ssd"})
        );
        assert!(
            body["operation_id"]
                .as_str()
                .unwrap()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
        assert!(
            !e.can_apply_look() && !e.can_edit_look(),
            "one apply at a time, no edits under it"
        );
        e.settled(ticket, reply(0, json!({"status":"changed","receipt":{}})));
        assert!(e.look.draft.is_none(), "applied edits retire on the answer");
        // Another writer changes the mode before the read: it shows as is.
        answer_next(
            &mut e,
            0,
            snapshot_with(
                "9",
                json!({"scheme":"forest","style":"pro","mode":"light","contrast":"normal","decorations":"ssd","source":null}),
            ),
        );
        assert!(e.look.draft.is_none() && e.look.conflicts.is_empty());
        assert_eq!(e.look.shown().unwrap().mode, Mode::Light);
    }

    #[test]
    fn the_scheme_s_own_style_is_sent_as_null() {
        let look = Look {
            style: Some(Style::Classic),
            ..Look::default()
        };
        let own = look.with(&json!({"style":"own"})).unwrap();
        assert_eq!(own.changes(&look)["appearance.style"], Value::Null);
    }

    #[test]
    fn another_writer_on_an_untouched_axis_is_followed_not_undone() {
        let mut e = read();
        edit(&mut e, json!({"mode":"light"}));
        // A hint: the scheme changed elsewhere.
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "ocean", "dark"));
        let draft = e.look.draft.unwrap();
        assert_eq!(
            draft.look.scheme,
            Scheme::Ocean,
            "the untouched axis follows"
        );
        assert_eq!(draft.look.mode, Mode::Light, "the edit stays");
        assert!(e.look.conflicts.is_empty() && e.can_apply_look());
        e.apply_look();
        let (_, _, body) = settings_calls(&mut e)[0].clone();
        assert_eq!(body["changes"], json!({"appearance.mode":"light"}));
        assert_eq!(body["expected_revision"], "8");
    }

    #[test]
    fn another_writer_on_an_edited_axis_must_be_reconciled() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "ocean", "dark"));
        assert_eq!(e.look.conflicts, vec![Axis::Scheme]);
        assert!(
            !e.can_apply_look(),
            "nothing is applied over an unseen change"
        );
        assert!(e.can_keep_look() && e.can_revert_look());
        e.keep_look();
        assert!(e.can_apply_look());
        e.apply_look();
        let (_, _, body) = settings_calls(&mut e)[0].clone();
        assert_eq!(body["changes"], json!({"appearance.scheme":"forest"}));
    }

    #[test]
    fn another_writer_agreeing_with_an_edit_retires_it() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        assert!(e.look.draft.is_none() && e.look.conflicts.is_empty());
    }

    #[test]
    fn a_refused_conflict_rereads_and_keeps_the_edits() {
        let mut e = read();
        edit(&mut e, json!({"mode":"light"}));
        e.apply_look();
        answer_next(
            &mut e,
            10,
            json!({"status":"conflict","incarnation":"inc-1","revision":"9"}),
        );
        assert!(e.failed);
        answer_next(&mut e, 0, snapshot_body("9", "ocean", "dark"));
        let draft = e.look.draft.unwrap();
        assert_eq!(
            (draft.look.scheme, draft.look.mode),
            (Scheme::Ocean, Mode::Light)
        );
        assert!(e.can_apply_look());
    }

    #[test]
    fn a_lost_apply_is_settled_by_its_receipt_before_anything_else() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        let calls = settings_calls(&mut e);
        let operation = calls[0].2["operation_id"].as_str().unwrap().to_owned();
        e.settled(calls[0].0, lost());
        assert!(e.look.uncertain.is_some() && !e.can_apply_look());
        let (verb, sent) = answer_next(
            &mut e,
            0,
            json!({"status":"current","receipt":{"operation_id":operation},"recovering":false}),
        );
        assert_eq!(
            (verb, sent["operation_id"].as_str()),
            ("settings.status", Some(operation.as_str()))
        );
        assert!(e.look.uncertain.is_none() && e.look.draft.is_none());
        assert_eq!(e.status, "applied-look");
        assert_eq!(settings_calls(&mut e)[0].1, "settings.get");
    }

    #[test]
    fn no_receipt_is_no_verdict_the_read_decides() {
        for (shows, status) in [(true, "applied-look"), (false, "look-unconfirmed")] {
            let mut e = read();
            edit(&mut e, json!({"scheme":"forest"}));
            e.apply_look();
            answer_next(
                &mut e,
                10,
                json!({"status":"outcome_unknown","message":"recovering"}),
            );
            answer_next(
                &mut e,
                0,
                json!({"status":"unknown_operation","receipt":null,"recovering":false}),
            );
            assert!(e.look.uncertain.is_none());
            let scheme = if shows { "forest" } else { "studio" };
            let (verb, _) = answer_next(&mut e, 0, snapshot_body("8", scheme, "dark"));
            assert_eq!(verb, "settings.get");
            assert_eq!(e.status, status);
            assert_eq!(
                e.look.draft.is_some(),
                !shows,
                "kept unless the session shows it"
            );
        }
    }

    #[test]
    fn a_recovering_settingsd_keeps_the_apply_uncertain_and_check_again_asks_again() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        let calls = settings_calls(&mut e);
        e.settled(calls[0].0, lost());
        answer_next(
            &mut e,
            0,
            json!({"status":"unknown_operation","receipt":null,"recovering":true}),
        );
        assert!(e.look.uncertain.is_some() && !e.can_apply_look());
        assert!(settings_calls(&mut e).is_empty());
        assert!(e.can_recheck_look());
        e.recheck_look();
        assert_eq!(settings_calls(&mut e)[0].1, "settings.status");
    }

    #[test]
    fn no_edit_is_taken_while_an_apply_is_unsettled() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        let calls = settings_calls(&mut e);
        let operation = calls[0].2["operation_id"].as_str().unwrap().to_owned();
        e.settled(calls[0].0, lost());
        assert!(!e.can_edit_look() && !e.can_revert_look());
        // Selecting Studio again during the receipt lookup is refused, not
        // silently folded into the old base.
        edit(&mut e, json!({"scheme":"studio"}));
        assert_eq!(e.look.draft.unwrap().look.scheme, Scheme::Forest);
        answer_next(
            &mut e,
            0,
            json!({"status":"current","receipt":{"operation_id":operation},"recovering":false}),
        );
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        assert!(e.can_edit_look());
        edit(&mut e, json!({"scheme":"studio"}));
        assert_eq!(
            e.look.draft.unwrap().look.scheme,
            Scheme::Studio,
            "settled: going back is an edit"
        );
    }

    #[test]
    fn edits_wait_for_the_read_after_a_successful_apply() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        answer_next(&mut e, 0, json!({"status":"changed","receipt":{}}));
        assert!(!e.can_edit_look(), "applied, not yet read back");
        assert!(e.set_look(&json!({"scheme":"studio"})).is_err());
        // The read-back fails: still waiting; Refresh reads again.
        let calls = settings_calls(&mut e);
        e.settled(calls[0].0, lost());
        assert!(!e.can_edit_look());
        e.ui.panel = crate::Panel::Appearance;
        assert!(e.can_refresh());
        e.refresh();
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        assert!(e.can_edit_look());
        edit(&mut e, json!({"scheme":"ocean"}));
        assert!(e.look.conflicts.is_empty(), "its own write is no conflict");
        assert_eq!(e.look.draft.unwrap().look.scheme, Scheme::Ocean);
    }

    #[test]
    fn a_failed_receipt_lookup_can_be_checked_again() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        let calls = settings_calls(&mut e);
        e.settled(calls[0].0, lost());
        let receipt = settings_calls(&mut e);
        e.settled(receipt[0].0, lost());
        assert!(e.look.uncertain.is_some());
        e.recheck_look();
        assert_eq!(settings_calls(&mut e)[0].1, "settings.status");
    }

    #[test]
    fn quitting_settles_a_lost_apply_by_receipt_a_bounded_number_of_times() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        e.apply_look();
        let apply = settings_calls(&mut e);
        e.quit();
        assert!(
            !e.take_effects().contains(&Effect::Exit),
            "waits for the apply"
        );
        e.settled(apply[0].0, lost());
        for _ in 0..QUIT_RECEIPT_TRIES {
            let calls: Vec<_> = e
                .take_effects()
                .into_iter()
                .filter_map(|x| match x {
                    Effect::Settings { ticket, verb, .. } => Some((ticket, verb)),
                    _ => None,
                })
                .collect();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, "settings.status");
            e.settled(calls[0].0, lost());
        }
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn a_hint_while_busy_rereads_once_afterwards() {
        let mut e = read();
        e.read_look();
        let first = settings_calls(&mut e);
        e.read_look();
        e.read_look();
        assert!(settings_calls(&mut e).is_empty());
        e.settled(first[0].0, reply(0, snapshot_body("7", "studio", "dark")));
        assert_eq!(settings_calls(&mut e).len(), 1);
    }

    #[test]
    fn a_late_reply_is_ignored() {
        let mut e = read();
        e.read_look();
        let calls = settings_calls(&mut e);
        e.settled(
            calls[0].0 + 100,
            reply(0, snapshot_body("99", "ocean", "dark")),
        );
        assert_eq!(e.look.revision(), Some(7));
    }

    #[test]
    fn missing_settingsd_reads_as_missing() {
        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("example");
        let calls = settings_calls(&mut e);
        e.settled(
            calls[0].0,
            Ok(Reply {
                rc: 10,
                body: String::new(),
                error: Some("Service 'settingsd' not found".into()),
            }),
        );
        assert_eq!(e.look.settings, Availability::Missing);
    }

    #[test]
    fn set_checks_every_axis_first() {
        let mut e = read();
        assert!(
            e.set_look(&json!({"scheme":"forest","mode":"dim"}))
                .is_err()
        );
        assert!(e.look.draft.is_none());
        let view = e
            .set_look(&json!({"scheme":"forest","caption_side":"left"}))
            .unwrap();
        assert_eq!(view["draft"]["scheme"], "forest");
        assert_eq!(view["draft"]["caption_side"], "left");
        assert_eq!(view["edits"], json!(["scheme", "caption_side"]));
    }
}
