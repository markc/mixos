// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.16: the MixOS app domains and overrides. `terminal.*`, `email.*` and
//! `ced.*` leaves are registry entries whose values their own modules check
//! (`terminal.rs`, `email.rs`, `content.rs`), unchanged: each domain's
//! stored leaves are laid onto its default record and the module validates
//! the whole record. `apps.<app>.*` are the per-app look overrides, flat;
//! `tune.*` the tunables catalogue (ADR R4), absorbed as two patterns.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind, Values};
use crate::content::{self, Ced};
use crate::email::{self, Email};
use crate::model::Diagnostic;
use crate::terminal::{self, Terminal};
use serde_json::Value;

const fn leaf(path: &'static str, page: &'static str) -> K {
    K::new(path, Kind::AppDomain, D::Domain)
        .on(page, &[Text, Layout, Paint])
        .mixos()
}
const fn term(path: &'static str) -> K {
    leaf(path, "applications/term")
}
const fn mail(path: &'static str) -> K {
    leaf(path, "applications/email")
}
const fn look(path: &'static str, kind: Kind) -> K {
    K::new(path, kind, D::Null)
        .on("applications/look", &[Paint, Text, Layout])
        .mixos()
}

pub(super) const KEYS: &[K] = &[
    term("terminal.font.family"),
    term("terminal.font.size"),
    term("terminal.font.weight"),
    term("terminal.padding"),
    term("terminal.scrollback.lines"),
    term("terminal.scroll.speed"),
    term("terminal.border.width"),
    term("terminal.cursor.shape"),
    term("terminal.cursor.blink"),
    term("terminal.focus.ring"),
    term("terminal.focus.strength"),
    term("terminal.mouse.copy_on_select"),
    term("terminal.clipboard.osc52_read"),
    term("terminal.env.term"),
    mail("email.accounts"),
    mail("email.default_account"),
    mail("email.mail.page_size"),
    mail("email.mail.mark_read_on_open"),
    mail("email.mail.confirm_delete"),
    mail("email.mail.quote_on_reply"),
    mail("email.mail.reply_position"),
    mail("email.reader.padding"),
    mail("email.reader.font.family"),
    mail("email.reader.font.size"),
    mail("email.reader.light"),
    mail("email.composer.padding"),
    mail("email.composer.font.family"),
    mail("email.composer.font.size"),
    mail("email.composer.light"),
    leaf("ced.editor.padding", "applications/ced"),
    leaf("ced.editor.font.family", "applications/ced"),
    leaf("ced.editor.font.size", "applications/ced"),
    leaf("ced.editor.light", "applications/ced"),
    look("apps.<app>.scheme", Kind::Enum(super::appearance::SCHEMES)),
    look("apps.<app>.mode", Kind::Enum(&["dark", "light"])),
    look("apps.<app>.contrast", Kind::Enum(&["normal", "high"])),
    look(
        "apps.<app>.text_scale",
        Kind::Number {
            min: 0.5,
            max: 3.0,
            step: 0.0,
        },
    ),
    // No domains of its own: each style token carries its own in
    // `design::tunables()` (`Tunable::domains`), which the change plan reads.
    K::new(
        "tune.style.<token>",
        Kind::Tunable,
        D::Owner("the design's token"),
    )
    .page("desktop/appearance/experimental/advanced")
    .mixos(),
    K::new(
        "tune.chrome.<role>",
        Kind::Tunable,
        D::Owner("the design's role"),
    )
    .on("desktop/appearance/experimental/advanced", &[Paint])
    .mixos(),
];

/// The three records, built from their stored leaves.
struct Records {
    terminal: Terminal,
    email: Email,
    ced: Ced,
}

impl Records {
    fn new() -> Self {
        Self {
            terminal: Terminal::default(),
            email: Email::default(),
            ced: Ced::default(),
        }
    }

    /// Lay one leaf onto its record with the module's own setter.
    fn set(&mut self, path: &str, value: &Value) -> Option<Result<(), Diagnostic>> {
        let (domain, rest) = path.split_once('.')?;
        let field: Vec<&str> = rest.split('.').collect();
        let value = Some(value.clone());
        Some(match domain {
            "terminal" => terminal::set(&mut self.terminal, path, &field, value),
            "email" => email::set(&mut self.email, path, &field, value),
            "ced" => content::set_ced(&mut self.ced, path, &field, value),
            _ => return None,
        })
    }

    /// Each record's first error: the modules stop at their first.
    fn validate(&self) -> Vec<Diagnostic> {
        [
            terminal::validate(&self.terminal),
            email::validate(&self.email),
            content::validate_ced(&self.ced),
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect()
    }

    fn is_default(&self) -> bool {
        self.terminal == Terminal::default()
            && self.email == Email::default()
            && self.ced == Ced::default()
    }
}

fn is_domain(path: &str) -> bool {
    ["terminal.", "email.", "ced."]
        .iter()
        .any(|p| path.starts_with(p))
}

/// Whether one leaf's value leaves its record at the default.
pub(super) fn is_default(path: &str, value: &Value) -> bool {
    let mut records = Records::new();
    matches!(records.set(path, value), Some(Ok(()))) && records.is_default()
}

/// Every stored domain leaf, laid onto its record, then each record checked
/// by its module. Every leaf that will not set is reported; the records are
/// checked only when every leaf set.
pub(super) fn check(values: &Values) -> Vec<Diagnostic> {
    let mut records = Records::new();
    let errors: Vec<Diagnostic> = values
        .iter()
        .filter(|(p, _)| is_domain(p))
        .filter_map(|(path, value)| records.set(path, value)?.err())
        .collect();
    if errors.is_empty() {
        records.validate()
    } else {
        errors
    }
}

/// Every leaf the modules describe, by path.
pub(super) fn schemas() -> serde_json::Map<String, Value> {
    let mut all = serde_json::Map::new();
    for group in [
        terminal::describe(),
        email::describe(),
        content::describe(content::CED_EDITOR),
    ] {
        if let Value::Object(group) = group {
            all.extend(group);
        }
    }
    all
}

/// The module's own describe entry for one leaf.
pub(super) fn schema(path: &str) -> Option<Value> {
    schemas().remove(path)
}
