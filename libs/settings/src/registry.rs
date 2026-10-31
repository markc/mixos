// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The settings key registry (key space v2): one declaration per key.
//!
//! Every key is a [`KeyDef`] in one static table per prefix: its kind and
//! range, its default, its class (stored, applied, live or state), its owner,
//! Prefs page and render domains. The store this registry validates is
//! [`Values`], a sparse map of authored non-default values keyed by full
//! dotted path. [`apply`] is the one generic write and [`validate`] the one
//! validator, both driven by the tables plus the registered cross-key
//! constraints ([`constraint`]); [`describe`] is generated from the same
//! tables.
//!
//! The v2 model is complete over [`Values`]: [`crate::view`] decodes them,
//! [`crate::resolve::values`] resolves them and
//! [`crate::domains::ChangePlan::of_values`] plans their changes.
//! settingsd's format-5 store holds [`Values`] and writes them through
//! [`apply`]; the frozen [`crate::v1`] only migrates format 4 and writes it
//! back for a downgrade.
use crate::model::Diagnostic;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

mod accessibility;
mod app_domains;
mod appearance;
mod apps;
pub mod constraint;
mod describe;
mod displays;
mod input;
mod keymaps;
mod kind;
mod network;
mod power;
mod shell;
mod sound;
mod state;
mod system;
#[cfg(test)]
pub(crate) mod tests;
mod time;
mod wallpaper;
mod window;
mod workspaces;

pub use describe::{describe, describe_key, label_id};

/// The authored values: full dotted path to value, only where it differs
/// from the registry default. Canonical order is the map's.
pub type Values = BTreeMap<String, Value>;

/// The registry's own version, reported by [`describe`].
pub const REGISTRY_VERSION: u32 = 2;
/// The contract version a key first shipped in, for every v2 key.
pub const SINCE: &str = "1.0.0";
/// Changes plus resets in one batch, all or nothing.
pub const MAX_BATCH: usize = 64;
/// The bound on `tune.*` entries.
pub const MAX_TUNE: usize = 128;

/// What a key is to settingsd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Authored in the sealed profile; a MixOS consumer reads it.
    Stored,
    /// Stored as above, and the named owner applies it to the system.
    Applied,
    /// Host state, not a setting: a catalogue entry only, never stored.
    Live,
    /// Internal, stored and sealed, written only by its named owner.
    State,
}

impl Class {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Applied => "applied",
            Self::Live => "live",
            Self::State => "state",
        }
    }
}

/// What a change to a key can move when it renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Paint,
    Text,
    Layout,
    Resources,
    Motion,
    Shell,
    Input,
    Export,
    System,
}

impl Domain {
    pub const ALL: [Self; 9] = [
        Self::Paint,
        Self::Text,
        Self::Layout,
        Self::Resources,
        Self::Motion,
        Self::Shell,
        Self::Input,
        Self::Export,
        Self::System,
    ];

    /// The domain named `name` (`paint`), as `design::tunables()` names them.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Paint => "paint",
            Self::Text => "text",
            Self::Layout => "layout",
            Self::Resources => "resources",
            Self::Motion => "motion",
            Self::Shell => "shell",
            Self::Input => "input",
            Self::Export => "export",
            Self::System => "system",
        }
    }
}

/// Whether a colour may carry an alpha byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    /// `#rrggbb` only.
    Opaque,
    /// `#rrggbb` or `#rrggbbaa`.
    Any,
}

/// The shape of a string value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grammar {
    /// A display name: no control characters, no surrounding space.
    Plain,
    /// A theme directory name: ASCII letters, digits, `.`, `_`, `-`, `+`.
    DirName,
    /// A DRM connector name.
    Connector,
    /// `WxH@R.RRR`.
    OutputMode,
    /// `x,y` in logical pixels.
    Position,
    /// An IANA zone name.
    Zone,
    /// A locale code (`en_AU.UTF-8`).
    Locale,
    /// A desktop-entry id (`firefox.desktop`).
    DesktopId,
    /// A MixOS applet id.
    Applet,
    /// A strict design source; resolution compiles it.
    Source,
    /// `#rrggbb` or `gradient:#rrggbb,#rrggbb@<deg>`.
    WallpaperColour,
}

impl Grammar {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::DirName => "directory_name",
            Self::Connector => "connector",
            Self::OutputMode => "output_mode",
            Self::Position => "position",
            Self::Zone => "iana_zone",
            Self::Locale => "locale",
            Self::DesktopId => "desktop_id",
            Self::Applet => "applet_id",
            Self::Source => "design_source",
            Self::WallpaperColour => "wallpaper_colour",
        }
    }
}

/// A key's value type. [`kind::check`] validates and normalises each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// A whole number in `min..=max` on the `step` grid from `min`.
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    /// A whole number from the set.
    OneOf(&'static [i64]),
    /// A number in `min..=max`; a `step` above 0 puts it on that grid.
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    /// One of the names.
    Enum(&'static [&'static str]),
    /// One of the known names, or an unknown plain name, which is retained.
    Known(&'static [&'static str]),
    Colour(Alpha),
    /// One of the words, or a colour.
    ColourOr(&'static [&'static str], Alpha),
    /// One of the words, or a connector name.
    ConnectorOr(&'static [&'static str]),
    /// A string of at most so many bytes, in the grammar.
    Str(Grammar, usize),
    /// An absolute path; `images` limits it to PNG, JPEG or WebP.
    Path {
        directory: bool,
        images: bool,
    },
    /// A list of `min..=max` items of the kind.
    List(&'static Kind, usize, usize),
    /// An xkb `[layout, variant]` pair.
    XkbLayout,
    /// Up to four chords in the keymap grammar.
    Chords,
    /// `{command: argv, bindings: chords}`.
    Spawn,
    /// A leaf of an app domain (terminal, email, ced), checked by its module.
    AppDomain,
    /// A `tune.*` entry, checked against `design::tunables()`.
    Tunable,
    /// A live catalogue entry: the type name of the owner's value.
    Catalogue(&'static str),
}

/// A key's default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DefaultValue {
    Null,
    Bool(bool),
    Int(i64),
    Number(f64),
    Text(&'static str),
    /// A JSON literal (lists).
    Json(&'static str),
    /// By the value of the first placeholder (panel ids).
    ByCapture(&'static [(&'static str, DefaultValue)]),
    /// No registry default: the named owner supplies it. A written value is
    /// always stored.
    Owner(&'static str),
    /// The app domain module's own default.
    Domain,
    /// Live entries have none.
    None,
}

/// One key: the single declaration every check, write and describe reads.
#[derive(Clone, Copy, Debug)]
pub struct KeyDef {
    /// Dotted path; `<name>` segments are placeholders ([`placeholder_ok`]).
    pub path: &'static str,
    pub kind: Kind,
    pub default: DefaultValue,
    pub class: Class,
    /// The applier (applied), the owner (live) or the writer (state).
    pub owner: Option<&'static str>,
    /// The owner's Bus commands that read and write a live key, where they
    /// exist today. None is structured "backend needed": the owner has no
    /// such command yet.
    pub read: Option<&'static str>,
    pub write: Option<&'static str>,
    pub domains: &'static [Domain],
    /// The Prefs page; none for state keys.
    pub page: Option<&'static str>,
    pub since: &'static str,
    /// No COSMIC counterpart.
    pub mixos_only: bool,
    /// Read only by the Mix exporters over the Bus.
    pub export_only: bool,
    /// When the value takes effect (recorded for Prefs; never refused).
    pub used_when: Option<&'static str>,
    /// Ids of the [`constraint::CONSTRAINTS`] that cover this key.
    pub constraints: &'static [&'static str],
}

impl KeyDef {
    pub const fn new(path: &'static str, kind: Kind, default: DefaultValue) -> Self {
        Self {
            path,
            kind,
            default,
            class: Class::Stored,
            owner: None,
            read: None,
            write: None,
            domains: &[],
            page: None,
            since: SINCE,
            mixos_only: false,
            export_only: false,
            used_when: None,
            constraints: &[],
        }
    }
    /// A live catalogue entry: never stored, refused by [`apply`].
    pub const fn live(path: &'static str, what: &'static str, owner: &'static str) -> Self {
        Self {
            class: Class::Live,
            owner: Some(owner),
            ..Self::new(path, Kind::Catalogue(what), DefaultValue::None)
        }
    }
    pub const fn on(self, page: &'static str, domains: &'static [Domain]) -> Self {
        Self {
            page: Some(page),
            domains,
            ..self
        }
    }
    pub const fn page(self, page: &'static str) -> Self {
        Self {
            page: Some(page),
            ..self
        }
    }
    pub const fn domains(self, domains: &'static [Domain]) -> Self {
        Self { domains, ..self }
    }
    pub const fn applied(self, owner: &'static str) -> Self {
        Self {
            class: Class::Applied,
            owner: Some(owner),
            ..self
        }
    }
    pub const fn state(self, writer: &'static str) -> Self {
        Self {
            class: Class::State,
            owner: Some(writer),
            page: None,
            ..self
        }
    }
    pub const fn reads(self, command: &'static str) -> Self {
        Self {
            read: Some(command),
            ..self
        }
    }
    pub const fn writes(self, command: &'static str) -> Self {
        Self {
            write: Some(command),
            ..self
        }
    }
    pub const fn mixos(self) -> Self {
        Self {
            mixos_only: true,
            ..self
        }
    }
    pub const fn export_only(self) -> Self {
        Self {
            export_only: true,
            ..self
        }
    }
    pub const fn used_when(self, text: &'static str) -> Self {
        Self {
            used_when: Some(text),
            ..self
        }
    }
    pub const fn constraints(self, ids: &'static [&'static str]) -> Self {
        Self {
            constraints: ids,
            ..self
        }
    }

    pub fn is_live(&self) -> bool {
        self.class == Class::Live
    }

    /// The default for this concrete key, or None where the registry has
    /// none (owner-supplied, domain, live).
    pub fn default_value(&self, captures: &[(&'static str, String)]) -> Option<Value> {
        default_json(self.default, captures)
    }
}

fn default_json(default: DefaultValue, captures: &[(&'static str, String)]) -> Option<Value> {
    match default {
        DefaultValue::Null => Some(Value::Null),
        DefaultValue::Bool(b) => Some(Value::Bool(b)),
        DefaultValue::Int(n) => Some(n.into()),
        DefaultValue::Number(n) => Some(n.into()),
        DefaultValue::Text(s) => Some(s.into()),
        DefaultValue::Json(text) => {
            Some(serde_json::from_str(text).expect("registry JSON default"))
        }
        DefaultValue::ByCapture(table) => {
            let first = captures.first().map(|(_, v)| v.as_str())?;
            let (_, default) = table.iter().find(|(name, _)| *name == first)?;
            default_json(*default, captures)
        }
        DefaultValue::Owner(_) | DefaultValue::Domain | DefaultValue::None => None,
    }
}

/// Every registry entry, in table order.
pub fn keys() -> &'static [KeyDef] {
    static ALL: OnceLock<Vec<KeyDef>> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            wallpaper::KEYS,
            appearance::KEYS,
            window::KEYS,
            shell::KEYS,
            workspaces::KEYS,
            displays::KEYS,
            sound::KEYS,
            power::KEYS,
            input::KEYS,
            keymaps::KEYS,
            apps::KEYS,
            time::KEYS,
            system::KEYS,
            accessibility::KEYS,
            network::KEYS,
            state::KEYS,
            app_domains::KEYS,
        ]
        .concat()
    })
}

/// The panel ids.
pub const PANELS: &[&str] = &["top", "bottom"];
/// The `apps_default.<category>` names.
pub const CATEGORIES: &[&str] = &[
    "web",
    "files",
    "mail",
    "music",
    "video",
    "photos",
    "calendar",
    "terminal",
    "text_editor",
];

/// The placeholders a path may use, each with what it accepts.
pub const PLACEHOLDERS: &[(&str, &str)] = &[
    (
        "<connector>",
        "a DRM connector: 1-32 ASCII letters, digits or -",
    ),
    ("<panel>", "top or bottom"),
    ("<tablet>", "a scoped identifier"),
    ("<app>", "a scoped identifier"),
    ("<name>", "a scoped identifier"),
    (
        "<action>",
        "a dotted action id: 1-64 ASCII letters, digits, ., - or _",
    ),
    ("<category>", "a default-application category"),
    ("<token>", "a style token of design::tunables()"),
    ("<role>", "a chrome role of design::tunables()"),
];

/// A display name, such as a font family: 1–128 bytes, no control
/// characters, no surrounding space. Whether it is installed is the
/// consumer's check.
pub fn plain_name(value: &str) -> bool {
    crate::terminal::valid_family(value)
}

/// A design appearance name (window style, menu side, caption side…):
/// 1–64 lowercase ASCII letters, digits or `-`. Unknown names are retained
/// for profiles shared with newer binaries.
pub fn appearance_name(value: &str) -> bool {
    design::WindowStyle::valid_name(value)
}

/// A scoped identifier, as today's model has it: 1–64 ASCII letters,
/// digits, `-` or `_`.
pub fn scoped_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A DRM connector name: 1–32 ASCII letters, digits or `-`.
pub fn connector(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Whether `value` fills `placeholder`.
pub fn placeholder_ok(placeholder: &str, value: &str) -> bool {
    match placeholder {
        "<connector>" => connector(value),
        "<panel>" => PANELS.contains(&value),
        "<tablet>" | "<app>" | "<name>" => scoped_id(value),
        "<action>" => crate::keymap::valid_action(value),
        "<category>" => CATEGORIES.contains(&value),
        "<token>" => tunable(&format!("style.{value}")).is_some(),
        "<role>" => tunable(&format!("chrome.{value}")).is_some(),
        _ => false,
    }
}

/// The tunable named `name` (`style.radius`, `chrome.accent`).
pub fn tunable(name: &str) -> Option<&'static design::Tunable> {
    design::tunables().iter().find(|t| t.name == name)
}

/// A concrete path matched to its entry, with each placeholder's value.
#[derive(Clone, Debug)]
pub struct Matched {
    pub def: &'static KeyDef,
    pub captures: Vec<(&'static str, String)>,
}

impl Matched {
    pub fn default_value(&self) -> Option<Value> {
        self.def.default_value(&self.captures)
    }
}

/// One entry's path, split once.
struct Pattern {
    def: &'static KeyDef,
    parts: Vec<&'static str>,
    literals: usize,
}

/// Every entry by its first segment, which is always literal.
fn index() -> &'static BTreeMap<&'static str, Vec<Pattern>> {
    static INDEX: OnceLock<BTreeMap<&'static str, Vec<Pattern>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index: BTreeMap<&'static str, Vec<Pattern>> = BTreeMap::new();
        for def in keys() {
            let parts: Vec<&'static str> = def.path.split('.').collect();
            let literals = parts.iter().filter(|p| !p.starts_with('<')).count();
            index.entry(parts[0]).or_default().push(Pattern {
                def,
                parts,
                literals,
            });
        }
        index
    })
}

enum Fit {
    No,
    /// The literal segments match but a placeholder value does not.
    Bad(String),
    Yes(Vec<(&'static str, String)>),
}

fn fit(parts: &[&'static str], segments: &[&str]) -> Fit {
    let mut captures = Vec::new();
    let mut rest = false;
    for (i, part) in parts.iter().enumerate() {
        // `<action>` takes the rest: action ids are dotted. It is always the
        // last part.
        if *part == "<action>" {
            if segments.len() <= i {
                return Fit::No;
            }
            captures.push((*part, segments[i..].join(".")));
            rest = true;
            break;
        }
        let Some(segment) = segments.get(i) else {
            return Fit::No;
        };
        if part.starts_with('<') {
            captures.push((*part, (*segment).to_owned()));
        } else if part != segment {
            return Fit::No;
        }
    }
    if !rest && segments.len() != parts.len() {
        return Fit::No;
    }
    // Every capture, the ones before a rest capture included.
    for (part, value) in &captures {
        if !placeholder_ok(part, value) {
            return Fit::Bad(format!("{value:?} is not a valid {part}"));
        }
    }
    Fit::Yes(captures)
}

/// The entry for a concrete path. Where several patterns fit, the one with
/// the most literal segments wins (`keymaps.comp.spawn.<name>` over
/// `keymaps.<app>.<action>`).
pub fn lookup(path: &str) -> Result<Matched, Diagnostic> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut best: Option<(usize, Matched)> = None;
    let mut reason = None;
    for pattern in index().get(segments[0]).into_iter().flatten() {
        match fit(&pattern.parts, &segments) {
            Fit::Yes(captures) => {
                if best.as_ref().is_none_or(|(n, _)| pattern.literals > *n) {
                    let def = pattern.def;
                    best = Some((pattern.literals, Matched { def, captures }));
                }
            }
            Fit::Bad(why) => reason = reason.or(Some(why)),
            Fit::No => {}
        }
    }
    best.map(|(_, m)| m).ok_or_else(|| {
        let code = if path.starts_with("tune.") {
            "unknown_tunable"
        } else {
            "validation_failed"
        };
        Diagnostic::new(
            code,
            path,
            reason.unwrap_or_else(|| "Unknown setting".into()),
        )
    })
}

/// The refusal for a live key: code `live_key`, and as its message the
/// structured owner and commands, `{"owner","read","write"}`, where a null
/// command is a backend the owner does not have yet.
fn live_refusal(def: &KeyDef, path: &str) -> Diagnostic {
    let detail = serde_json::json!({"owner": def.owner, "read": def.read, "write": def.write});
    Diagnostic::new("live_key", path, detail.to_string())
}

/// Whether a normalised value equals the key's default, so it is not stored.
pub(crate) fn is_default(m: &Matched, path: &str, value: &Value) -> bool {
    match m.def.default {
        DefaultValue::Domain => app_domains::is_default(path, value),
        _ => m.default_value().as_ref() == Some(value),
    }
}

/// An app domain leaf's own default, as its module describes it; None where
/// the module gives none (a family that falls back to the desktop's).
pub fn domain_default(path: &str) -> Option<Value> {
    app_domains::schema(path)?.get("default").cloned()
}

/// Validate and normalise one authored value for its key.
pub fn check_value(path: &str, value: &Value) -> Result<Value, Diagnostic> {
    let m = lookup(path)?;
    if m.def.is_live() {
        return Err(live_refusal(m.def, path));
    }
    kind::check(&m.def.kind, path, value)
}

/// The value of a key: authored, else the registry default. None for live
/// keys and owner-supplied defaults that were never authored.
pub fn effective(values: &Values, path: &str) -> Option<Value> {
    if let Some(value) = values.get(path) {
        return Some(value.clone());
    }
    lookup(path).ok()?.default_value()
}

/// The one generic write: resets (exact paths, or a prefix ending `.*`),
/// then changes, where `null` resets as a reset entry does; a value equal
/// to its default is removed, not stored. All or nothing: any diagnostic
/// refuses the batch.
pub fn apply(
    current: &Values,
    changes: &BTreeMap<String, Value>,
    reset: &[String],
) -> Result<Values, Vec<Diagnostic>> {
    // Counted together, as today's `resolve::patch` counts them.
    if changes.len() + reset.len() > MAX_BATCH {
        return Err(vec![Diagnostic::new(
            "validation_failed",
            "changes",
            "Batch limit exceeded",
        )]);
    }
    let mut next = current.clone();
    let mut errors = Vec::new();
    for path in reset {
        if changes.contains_key(path) {
            errors.push(Diagnostic::new(
                "validation_failed",
                path,
                "Cannot apply and reset the same field",
            ));
            continue;
        }
        if let Some(prefix) = path.strip_suffix(".*") {
            let under = under_prefix(prefix);
            if under.is_empty() {
                errors.push(Diagnostic::new(
                    "validation_failed",
                    path,
                    "Unknown settings prefix",
                ));
                continue;
            }
            // A prefix of live keys only (`system.*`) names no setting.
            if under.iter().all(|d| d.is_live()) {
                errors.push(live_refusal(under[0], path));
                continue;
            }
            let under = format!("{prefix}.");
            next.retain(|key, _| !key.starts_with(&under));
            continue;
        }
        match lookup(path) {
            Ok(m) if m.def.is_live() => errors.push(live_refusal(m.def, path)),
            Ok(_) => {
                next.remove(path);
            }
            Err(e) => errors.push(e),
        }
    }
    for (path, value) in changes {
        let m = match lookup(path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        if m.def.is_live() {
            errors.push(live_refusal(m.def, path));
            continue;
        }
        if value.is_null() {
            next.remove(path);
            continue;
        }
        match kind::check(&m.def.kind, path, value) {
            Ok(value) if is_default(&m, path, &value) => {
                next.remove(path);
            }
            Ok(value) => {
                next.insert(path.clone(), value);
            }
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    validate(&next)?;
    // Write-only constraints refuse only what this batch introduces, so a
    // stored conflict never blocks an unrelated write.
    let before = constraint::conflicts(current);
    let introduced: Vec<Diagnostic> = constraint::conflicts(&next)
        .into_iter()
        .filter(|d| !before.contains(d))
        .collect();
    if !introduced.is_empty() {
        return Err(introduced);
    }
    Ok(next)
}

/// Whether `prefix` names whole leading segments of some key
/// (`appearance`, `shell.panels.top`, `appearance.frosted`).
pub fn is_prefix(prefix: &str) -> bool {
    !under_prefix(prefix).is_empty()
}

/// The entries under `prefix`.
fn under_prefix(prefix: &str) -> Vec<&'static KeyDef> {
    let segments: Vec<&str> = prefix.split('.').collect();
    index()
        .get(segments[0])
        .into_iter()
        .flatten()
        .filter(|p| {
            segments.len() < p.parts.len()
                && segments.iter().zip(&p.parts).all(|(segment, part)| {
                    if part.starts_with('<') {
                        placeholder_ok(part, segment)
                    } else {
                        part == segment
                    }
                })
        })
        .map(|p| p.def)
        .collect()
}

/// The one validator, for load as for write: every stored key known, not
/// live, canonical and not its default; the family limits; the app domains;
/// the constraints except the write-only ones, which [`apply`] checks and
/// [`constraint::conflicts`] reports.
pub fn validate(values: &Values) -> Result<(), Vec<Diagnostic>> {
    let mut errors = Vec::new();
    for (path, value) in values {
        let m = match lookup(path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        if m.def.is_live() {
            errors.push(live_refusal(m.def, path));
            continue;
        }
        match kind::check(&m.def.kind, path, value) {
            Ok(normal) if normal != *value => errors.push(Diagnostic::new(
                "validation_failed",
                path,
                "Stored value is not in canonical form",
            )),
            Ok(_) if value.is_null() || is_default(&m, path, value) => errors.push(
                Diagnostic::new("validation_failed", path, "Stored value equals its default"),
            ),
            Ok(_) => {}
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    errors.extend(limits(values));
    errors.extend(app_domains::check(values));
    errors.extend(constraint::check(values));
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The bounded pattern families: (family, placeholder segment index, limit).
const FAMILIES: &[(&str, usize, usize)] = &[
    ("displays", 1, 16),
    ("wallpaper.outputs", 2, 16),
    ("input.tablet", 2, 16),
    ("apps", 1, 16),
    ("keymaps", 1, crate::keymap::MAX_APPS),
    ("keymaps.comp.spawn", 3, 32),
];

fn limits(values: &Values) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for (family, index, limit) in FAMILIES {
        let under = format!("{family}.");
        let members: BTreeSet<&str> = values
            .keys()
            .filter(|k| k.starts_with(&under))
            .filter_map(|k| k.split('.').nth(*index))
            .collect();
        if members.len() > *limit {
            errors.push(Diagnostic::new(
                "validation_failed",
                family,
                format!("At most {limit} entries"),
            ));
        }
    }
    let mut actions: BTreeMap<&str, usize> = BTreeMap::new();
    for key in values.keys() {
        if let Some(rest) = key.strip_prefix("keymaps.")
            && let Some((app, _)) = rest.split_once('.')
        {
            *actions.entry(app).or_default() += 1;
        }
    }
    for (app, count) in actions {
        if count > crate::keymap::MAX_ACTIONS {
            errors.push(Diagnostic::new(
                "validation_failed",
                &format!("keymaps.{app}"),
                "Collection limit exceeded",
            ));
        }
    }
    if values.keys().filter(|k| k.starts_with("tune.")).count() > MAX_TUNE {
        errors.push(Diagnostic::new(
            "validation_failed",
            "tune",
            format!("At most {MAX_TUNE} entries"),
        ));
    }
    errors
}
