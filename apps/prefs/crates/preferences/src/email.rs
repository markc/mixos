// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! App Settings → Email: the email app's accounts and mail options
//! (`email.*`), edited in settingsd as the Terminal group edits
//! `terminal.*`. Each change is one fenced `settings.apply` on the
//! Appearance panel's channel carrying the whole new `email.accounts` list,
//! with `email.default_account` in the same batch whenever the change
//! touches it, so no state between the two is ever stored. The values shown
//! are the ones settingsd last read; Prefs never writes an email config
//! file. A change is checked by the settings crate's own rules before it is
//! sent, and a refused one leaves what settingsd held.
//!
//! **Passwords never enter settingsd, nor Prefs' state.** Set password hands
//! the typed password to the email app's write-only
//! `email.account.password.set`, launching Email first (`apps.launch`) when
//! it is not on the Bus and waiting a bounded time for it to register. The
//! password is a [`Secret`]: never serialised, redacted in `Debug`, zeroed
//! when dropped, and put into the call's body only by the shell, as it sends.
//! Whether an account has a password is read from `email.accounts`.
use crate::appearance::{Answer, Call};
use crate::engine::{Dialog, Effect, Engine};
use crate::navigation::Section;
use citizen::{CallError, Reply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub use settings::email::{
    AccountEntry, Email, MailOptions, PAGE_SIZE_MAX, PAGE_SIZE_MIN, ReplyPosition, mint_account_id,
};

/// The email app's Bus service.
pub const EMAIL: &str = "email";
/// The service that launches applications.
pub const APPS: &str = "apps";
/// The email app's id, as `apps.launch` takes it.
pub const EMAIL_APP_ID: &str = "dev.mixos.email";
/// How long to wait for Email to register after launching it, one wait per
/// attempt (about 16 s in all). A registration on the Bus ends a wait early.
pub const LAUNCH_WAITS_MS: [u64; 6] = [250, 500, 1000, 2000, 4000, 8000];

/// A password on its way to the email app. It is never serialised, prints
/// as `Secret(..)`, and its bytes are overwritten when it is dropped.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The password itself, for the field that edits it and the call that
    /// sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

/// One field of the account form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Name,
    Url,
    Email,
    CaPath,
    Signature,
}

impl Field {
    pub const ALL: [Field; 5] = [
        Field::Name,
        Field::Url,
        Field::Email,
        Field::CaPath,
        Field::Signature,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::Url => "url",
            Field::Email => "email",
            Field::CaPath => "ca_path",
            Field::Signature => "signature",
        }
    }

    /// Why settingsd refuses this field, as a label key.
    pub fn refusal(self) -> &'static str {
        match self {
            Field::Name => "email-name-refused",
            Field::Url => "email-url-refused",
            Field::Email => "email-address-refused",
            Field::CaPath => "email-ca-refused",
            Field::Signature => "email-signature-refused",
        }
    }
}

/// The fields' refusals, by field.
pub type Refusals = BTreeMap<Field, &'static str>;

/// New values for some of an account's fields; `None` leaves a field.
/// Debug-printed with its URL redacted.
#[derive(Clone, Default, PartialEq)]
pub struct Patch {
    pub name: Option<String>,
    pub url: Option<String>,
    pub email: Option<String>,
    /// `Some(None)` is the system roots.
    pub ca_path: Option<Option<String>>,
    pub signature: Option<String>,
    pub enabled: Option<bool>,
}

impl Patch {
    /// This patch laid over `account`, then checked field by field.
    fn over(&self, mut account: AccountEntry) -> Result<AccountEntry, Refusals> {
        if let Some(name) = &self.name {
            account.name = name.trim().to_owned();
        }
        if let Some(url) = &self.url {
            account.url = url.trim().to_owned();
        }
        if let Some(email) = &self.email {
            account.email = email.trim().to_owned();
        }
        if let Some(ca) = &self.ca_path {
            account.ca_path = ca
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
        }
        if let Some(signature) = &self.signature {
            account.signature = signature.clone();
        }
        if let Some(enabled) = self.enabled {
            account.enabled = enabled;
        }
        let refusals = refusals(&account);
        if refusals.is_empty() {
            Ok(account)
        } else {
            Err(refusals)
        }
    }

    /// The patch an object of `name`, `url`, `email`, `ca_path` (a path, or
    /// null for the system roots), `signature` and `enabled` asks for; any
    /// other key but `op` and `id` is refused.
    pub fn from_args(args: &Value) -> Result<Self, String> {
        let text = |key: &str| -> Result<Option<String>, String> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(format!("{key} must be a string")),
            }
        };
        if let Some(object) = args.as_object()
            && let Some(key) = object.keys().find(|k| {
                !matches!(
                    k.as_str(),
                    "op" | "id" | "name" | "url" | "email" | "ca_path" | "signature" | "enabled"
                )
            })
        {
            return Err(format!("unknown account key {key:?}"));
        }
        let ca_path = match args.get("ca_path") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(path)) => Some(Some(path.clone())),
            Some(_) => return Err("ca_path must be a path or null".into()),
        };
        let enabled = match args.get("enabled") {
            None => None,
            Some(Value::Bool(on)) => Some(*on),
            Some(_) => return Err("enabled must be true or false".into()),
        };
        Ok(Self {
            name: text("name")?,
            url: text("url")?,
            email: text("email")?,
            ca_path,
            signature: text("signature")?,
            enabled,
        })
    }
}

/// Every field of `account` settingsd would refuse, by the settings crate's
/// rules.
pub fn refusals(account: &AccountEntry) -> Refusals {
    use settings::email::{valid_address, valid_ca_path, valid_name, valid_signature, valid_url};
    let mut out = Refusals::new();
    let checks = [
        (Field::Name, valid_name(&account.name)),
        (Field::Url, valid_url(&account.url)),
        (Field::Email, valid_address(&account.email)),
        (
            Field::CaPath,
            account.ca_path.as_deref().is_none_or(valid_ca_path),
        ),
        (Field::Signature, valid_signature(&account.signature)),
    ];
    for (field, ok) in checks {
        if !ok {
            out.insert(field, field.refusal());
        }
    }
    out
}

/// The text that stands in for a credential in anything observed.
pub const REDACTED: &str = "[redacted]";

/// Query keys whose values read as credentials.
const SECRET_KEYS: [&str; 7] = [
    "pass",
    "pwd",
    "secret",
    "token",
    "key",
    "auth",
    "credential",
];

/// `url` as it may be observed: userinfo (`user:password@`) before the host
/// becomes [`REDACTED`]`@`, and so does the value of any query parameter
/// whose name reads as a credential (`password=`, `token=`, `api_key=`…).
/// Only the edit field holds what was typed; `prefs.email`, `prefs.info`,
/// Debug and status text show this.
pub fn redact_url(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(at) => url.split_at(at + 3),
        None => ("", url),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let authority = match authority.rfind('@') {
        Some(at) => format!("{REDACTED}{}", &authority[at..]),
        None => authority.to_owned(),
    };
    let tail = match tail.split_once('?') {
        None => tail.to_owned(),
        Some((path, query)) => {
            let (query, fragment) = match query.split_once('#') {
                Some((q, f)) => (q, Some(f)),
                None => (query, None),
            };
            let pairs: Vec<String> = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some((key, _)) if credential_key(key) => format!("{key}={REDACTED}"),
                    _ => pair.to_owned(),
                })
                .collect();
            let fragment = fragment.map(|f| format!("#{f}")).unwrap_or_default();
            format!("{path}?{}{fragment}", pairs.join("&"))
        }
    };
    format!("{scheme}{authority}{tail}")
}

fn credential_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|k| key.contains(k))
}

/// Free text (a refusal's message, a diagnostic) as it may be observed:
/// each URL in it (a word with `://`) passes through [`redact_url`].
pub fn redact_text(text: &str) -> String {
    if !text.contains("://") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.contains("://") {
            out.push_str(&redact_url(word));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '`') {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A settingsd or Email refusal's text, redacted.
fn refusal(value: &Value) -> String {
    redact_text(&crate::appearance::refusal(value))
}

fn serialize_redacted<S: serde::Serializer>(url: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&redact_url(url))
}

/// The account form: the selected account's text as typed, or a new
/// account's. Its fields go out together with Save. Serialised and
/// Debug-printed with its URL redacted ([`redact_url`]): only the edit
/// field shows the URL as typed.
#[derive(Clone, Default, PartialEq, Serialize)]
pub struct Form {
    /// The account it edits; `None` is a new account.
    pub id: Option<String>,
    pub name: String,
    #[serde(serialize_with = "serialize_redacted")]
    pub url: String,
    pub email: String,
    /// Blank is the system roots.
    pub ca_path: String,
    pub signature: String,
    /// Why the last Save was not sent, by field.
    pub errors: Refusals,
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &redact_url(&self.url))
            .field("email", &self.email)
            .field("ca_path", &self.ca_path)
            .field("signature", &self.signature)
            .field("errors", &self.errors)
            .finish()
    }
}

impl std::fmt::Debug for Patch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Patch")
            .field("name", &self.name)
            .field("url", &self.url.as_deref().map(redact_url))
            .field("email", &self.email)
            .field("ca_path", &self.ca_path)
            .field("signature", &self.signature)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl Form {
    /// A blank form for a new account.
    pub fn blank() -> Self {
        Self::default()
    }

    /// The form showing `account` as settingsd holds it.
    pub fn of(account: &AccountEntry) -> Self {
        Self {
            id: Some(account.id.clone()),
            name: account.name.clone(),
            url: account.url.clone(),
            email: account.email.clone(),
            ca_path: account.ca_path.clone().unwrap_or_default(),
            signature: account.signature.clone(),
            errors: Refusals::new(),
        }
    }

    pub fn get(&self, field: Field) -> &str {
        match field {
            Field::Name => &self.name,
            Field::Url => &self.url,
            Field::Email => &self.email,
            Field::CaPath => &self.ca_path,
            Field::Signature => &self.signature,
        }
    }

    fn slot(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.name,
            Field::Url => &mut self.url,
            Field::Email => &mut self.email,
            Field::CaPath => &mut self.ca_path,
            Field::Signature => &mut self.signature,
        }
    }

    /// Every field as a patch: what Save sends.
    pub fn patch(&self) -> Patch {
        Patch {
            name: Some(self.name.clone()),
            url: Some(self.url.clone()),
            email: Some(self.email.clone()),
            ca_path: Some(Some(self.ca_path.clone())),
            signature: Some(self.signature.clone()),
            enabled: None,
        }
    }

    /// The text differs from `saved` (a new form differs from nothing only
    /// once something is typed).
    pub fn edited(&self, saved: Option<&AccountEntry>) -> bool {
        match saved {
            Some(saved) => {
                let shown = Form::of(saved);
                Field::ALL.iter().any(|f| self.get(*f) != shown.get(*f))
            }
            None => Field::ALL.iter().any(|f| !self.get(*f).is_empty()),
        }
    }
}

/// Which way an account moves in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// One change to the email settings, as the window and `prefs.email.set`
/// make it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A new account at the end of the list; its id is minted.
    Add(Patch),
    Update(String, Patch),
    /// Clears the default too when it named this account.
    Remove(String),
    Move(String, Direction),
    /// `None` sends from the first enabled account.
    Default(Option<String>),
    Enable(String, bool),
    PageSize(u32),
    MarkReadOnOpen(bool),
    ConfirmDelete(bool),
    /// Whether a plain Reply quotes the original message.
    QuoteOnReply(bool),
    /// Where the person's text goes in a quoted reply.
    ReplyPosition(settings::email::ReplyPosition),
    /// The whole identity list of one account; an empty list makes it send
    /// as itself.
    Identities(String, Vec<settings::email::Identity>),
}

/// One text field of an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityField {
    Name,
    Address,
    Signature,
    ReplyTo,
}

/// The identity list being edited for one account, before it is saved.
#[derive(Clone, Debug, PartialEq)]
pub struct IdentityDraft {
    pub account: String,
    pub rows: Vec<settings::email::Identity>,
    pub selected: usize,
    /// Why the last save was refused.
    pub error: Option<Refusal>,
}

/// Why a change was not sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// Fields settingsd would refuse.
    Fields(Refusals),
    /// A label key: no such account, the list is full, a value out of range.
    Key(&'static str),
    /// The settings crate's own check of the whole group.
    Settings(String),
    /// It changes nothing.
    Unchanged,
}

/// What one change asks of settingsd: the paths it sets, in one fenced
/// batch, or the whole group back to its defaults.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Apply(BTreeMap<String, Value>),
    Reset,
}

/// A change made sendable: its edit, and the id it gives a new account.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub edit: Edit,
    pub added: Option<String>,
}

/// The edit taking `old` to `new`: each path that differs, the account list
/// whole.
fn edit_between(old: &Email, new: &Email) -> BTreeMap<String, Value> {
    let mut changes = BTreeMap::new();
    if old.accounts != new.accounts {
        changes.insert("email.accounts".into(), json!(new.accounts));
    }
    if old.default_account != new.default_account {
        changes.insert("email.default_account".into(), json!(new.default_account));
    }
    let (a, b) = (&old.mail, &new.mail);
    if a.page_size != b.page_size {
        changes.insert("email.mail.page_size".into(), json!(b.page_size));
    }
    if a.mark_read_on_open != b.mark_read_on_open {
        changes.insert(
            "email.mail.mark_read_on_open".into(),
            json!(b.mark_read_on_open),
        );
    }
    if a.confirm_delete != b.confirm_delete {
        changes.insert("email.mail.confirm_delete".into(), json!(b.confirm_delete));
    }
    if a.quote_on_reply != b.quote_on_reply {
        changes.insert("email.mail.quote_on_reply".into(), json!(b.quote_on_reply));
    }
    if a.reply_position != b.reply_position {
        changes.insert("email.mail.reply_position".into(), json!(b.reply_position));
    }
    changes
}

/// `change` over `email` as settingsd holds it: the edit to send, checked
/// by the settings crate's rules. `mint` gives a new account its id.
pub fn plan(
    email: &Email,
    change: &Change,
    mint: impl FnOnce(&[AccountEntry]) -> String,
) -> Result<Planned, Refusal> {
    let mut next = email.clone();
    let mut added = None;
    let index = |id: &str| {
        next.accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or(Refusal::Key("email-unknown-account"))
    };
    match change {
        Change::Add(patch) => {
            if next.accounts.len() >= settings::email::ACCOUNTS_MAX {
                return Err(Refusal::Key("email-too-many"));
            }
            let id = mint(&next.accounts);
            let blank = AccountEntry {
                id: id.clone(),
                name: String::new(),
                url: String::new(),
                email: String::new(),
                ca_path: None,
                enabled: true,
                signature: String::new(),
                identities: Vec::new(),
            };
            let account = patch.over(blank).map_err(Refusal::Fields)?;
            next.accounts.push(account);
            added = Some(id);
        }
        Change::Update(id, patch) => {
            let at = index(id)?;
            next.accounts[at] = patch
                .over(next.accounts[at].clone())
                .map_err(Refusal::Fields)?;
        }
        Change::Remove(id) => {
            let at = index(id)?;
            next.accounts.remove(at);
            if next.default_account.as_deref() == Some(id.as_str()) {
                next.default_account = None;
            }
        }
        Change::Move(id, direction) => {
            let at = index(id)?;
            let to = match direction {
                Direction::Up => at.checked_sub(1),
                Direction::Down => Some(at + 1).filter(|to| *to < next.accounts.len()),
            };
            let Some(to) = to else {
                return Err(Refusal::Unchanged);
            };
            next.accounts.swap(at, to);
        }
        Change::Default(id) => {
            if let Some(id) = id {
                index(id)?;
            }
            next.default_account = id.clone();
        }
        Change::Enable(id, enabled) => {
            let at = index(id)?;
            next.accounts[at].enabled = *enabled;
        }
        Change::PageSize(size) => {
            if !(PAGE_SIZE_MIN..=PAGE_SIZE_MAX).contains(size) {
                return Err(Refusal::Key("email-page-size-refused"));
            }
            next.mail.page_size = *size;
        }
        Change::MarkReadOnOpen(on) => next.mail.mark_read_on_open = *on,
        Change::ConfirmDelete(on) => next.mail.confirm_delete = *on,
        Change::QuoteOnReply(on) => next.mail.quote_on_reply = *on,
        Change::ReplyPosition(position) => next.mail.reply_position = *position,
        Change::Identities(id, identities) => {
            let at = index(id)?;
            next.accounts[at].identities = identities.clone();
        }
    }
    settings::email::validate(&next).map_err(|d| Refusal::Settings(redact_text(&d.message)))?;
    let changes = edit_between(email, &next);
    if changes.is_empty() {
        return Err(Refusal::Unchanged);
    }
    Ok(Planned {
        edit: Edit::Apply(changes),
        added,
    })
}

/// The email app on the Bus, as Prefs last saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    /// Not asked yet.
    #[default]
    Unknown,
    /// It answered `email.accounts`.
    Running,
    /// Not registered on the Bus.
    Absent,
    /// It does not answer `email.accounts` (an older Email).
    Unsupported,
}

/// Something to ask the email app for one account.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Password(String, Secret),
    /// Delete the account's saved password: `email.account.password.set`
    /// with an empty password, the only path that sends one.
    Forget(String),
    Connect(String),
    /// Remove the account through Email (`email.account.remove`), which
    /// refuses while it migrates and forgets the password itself.
    Remove(String),
}

impl Action {
    fn id(&self) -> &str {
        match self {
            Action::Password(id, _)
            | Action::Forget(id)
            | Action::Connect(id)
            | Action::Remove(id) => id,
        }
    }

    fn verb(&self) -> &'static str {
        match self {
            Action::Password(..) | Action::Forget(_) => "email.account.password.set",
            Action::Remove(_) => "email.account.remove",
            Action::Connect(_) => "email.account.connect",
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Action::Password(..) => "password",
            Action::Forget(_) => "forget",
            Action::Remove(_) => "remove",
            Action::Connect(_) => "connect",
        }
    }
}

/// Where an action stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Its call to Email is in flight.
    Sending,
    /// Email was not on the Bus: `apps.launch` is in flight.
    Launching,
    /// Email was launched: waiting for it to register.
    Waiting,
}

/// The action in progress: one at a time.
#[derive(Clone, Debug)]
pub struct Task {
    pub action: Action,
    pub stage: Stage,
    ticket: u64,
    /// Email has been launched for it.
    launched: bool,
    /// Waits spent of [`LAUNCH_WAITS_MS`].
    waits: usize,
    /// The `prefs.email.password` command that asked, answered when it ends.
    reply: Option<u64>,
    /// Email may be launched for it; a forget after a removal only tries an
    /// Email already running.
    may_launch: bool,
    /// The account's name when the action began (a removed account has
    /// none to look up afterwards).
    name: String,
}

/// The Email section's own state; the settings themselves are
/// `look.email`, as settingsd last read them.
#[derive(Clone, Debug, Default)]
pub struct EmailEditor {
    /// The selected account's form, or a new account's.
    pub form: Option<Form>,
    /// The id a sent Add gives its account: once a read shows it, the form
    /// is that account's.
    added: Option<String>,
    /// The password typed for the selected account, not yet sent.
    pub password: Secret,
    pub service: Service,
    /// `has_password` per account, from Email's last `email.accounts`.
    pub passwords: BTreeMap<String, bool>,
    accounts_job: Option<u64>,
    pub task: Option<Task>,
    /// How the last password or connect ended, and whether it failed.
    pub outcome: Option<(String, bool)>,
    /// A sent Remove, by id and name: once a read shows it gone, Email is
    /// asked to forget the account's password.
    removed: Option<(String, String)>,
    /// Forgets waiting for the action in progress to end.
    forgets: Vec<(String, String)>,
    /// The identity list being edited, for the selected account.
    pub identities: Option<IdentityDraft>,
}

/// The `has_password` per account in an `email.accounts` answer: a list of
/// accounts, or an object holding one as `accounts`.
fn passwords(value: &Value) -> Option<BTreeMap<String, bool>> {
    let list = value.as_array().or_else(|| value["accounts"].as_array())?;
    Some(
        list.iter()
            .filter_map(|a| Some((a["id"].as_str()?.to_owned(), a["has_password"].as_bool()?)))
            .collect(),
    )
}

/// The email app's answer refuses an unknown verb.
fn unknown_verb(value: &Value) -> bool {
    value["error_code"] == "UNKNOWN_VERB"
        || value["message"]
            .as_str()
            .is_some_and(|m| m.to_ascii_lowercase().contains("unknown verb"))
}

impl Engine {
    /// The email settings as settingsd last read them.
    pub fn email(&self) -> Option<&Email> {
        self.look.email.as_ref()
    }

    /// The selected account as settingsd holds it.
    pub fn email_account(&self) -> Option<&AccountEntry> {
        let id = self.ui.email_account.as_deref()?;
        self.email()?.account(id)
    }

    /// The Email section takes changes: settingsd read, a fence to send
    /// with, and no dialog, no quit under way.
    pub fn can_edit_email(&self) -> bool {
        self.look.email.is_some()
            && self.look.terminal_ready()
            && self.connected
            && self.ui.dialog.is_none()
            && !self.picker.open
            && !self.quitting
    }

    /// Plan `change` and send it: one fenced apply. A change settingsd
    /// would refuse is not sent; the status (and, for fields, the form)
    /// says why.
    pub fn change_email(&mut self, change: Change) -> Result<Planned, Refusal> {
        if !self.can_edit_email() {
            return Err(Refusal::Key("busy"));
        }
        let email = self.look.email.clone().unwrap_or_default();
        let planned = match plan(&email, &change, mint_account_id) {
            Ok(planned) => planned,
            Err(refusal) => {
                self.email_refused(&change, &refusal);
                return Err(refusal);
            }
        };
        if let Some(form) = &mut self.email.form
            && matches!(change, Change::Add(_) | Change::Update(..))
        {
            form.errors.clear();
        }
        self.email.added = planned.added.clone();
        self.email.removed = match &change {
            Change::Remove(id) => Some((id.clone(), self.account_name(id))),
            _ => None,
        };
        self.send_email(planned.edit.clone());
        Ok(planned)
    }

    fn email_refused(&mut self, change: &Change, refusal: &Refusal) {
        match refusal {
            Refusal::Fields(fields) => {
                if let Some(form) = &mut self.email.form
                    && matches!(change, Change::Add(_) | Change::Update(..))
                {
                    form.errors = fields.clone();
                }
                self.status = self.label("email-fields-refused", &[]);
                self.failed = true;
            }
            Refusal::Key(key) => {
                self.status = self.label(key, &[]);
                self.failed = true;
            }
            Refusal::Settings(message) => {
                self.status = self.label("email-failed", &[("message", message)]);
                self.failed = true;
            }
            Refusal::Unchanged => {
                self.status = self.label("email-unchanged", &[]);
                self.failed = false;
            }
        }
    }

    fn send_email(&mut self, edit: Edit) {
        let operation = self.look.operation();
        self.status = self.label("applying-email", &[]);
        self.failed = false;
        self.call_settings(Call::Email(operation, edit));
    }

    /// An email apply settled. Whatever the answer, settingsd is read again
    /// before the next one: the read shows whether it landed and gives the
    /// fence for the next. A refused one keeps what settingsd held, and a
    /// refused Add keeps its form.
    pub(crate) fn email_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(_) => {
                self.status = self.label("applied-email", &[]);
                self.failed = false;
            }
            Answer::Refused(value) if value["status"] == "conflict" => {
                self.email.added = None;
                self.email.removed = None;
                self.status = self.label("email-conflict", &[]);
                self.failed = true;
            }
            Answer::Refused(value) => {
                self.email.added = None;
                self.email.removed = None;
                let message = refusal(&value);
                self.status = self.label("email-failed", &[("message", message.as_str())]);
                self.failed = true;
            }
            Answer::NotSent(message) => {
                self.email.added = None;
                self.email.removed = None;
                self.status = self.label("email-failed", &[("message", message.as_str())]);
                self.failed = true;
            }
            // A lost reply is no verdict: the read shows whether the Add or
            // Remove landed, so its id is kept until then.
            Answer::Lost(message) => {
                self.status = self.label("email-failed", &[("message", message.as_str())]);
                self.failed = true;
            }
            Answer::Missing => return,
        }
        self.look.settle_by_read();
    }

    /// A read brought settingsd's email settings: the form follows them
    /// unless the person has typed into it, a sent Add becomes its
    /// account's form, and an account gone from the list leaves the form.
    pub(crate) fn email_read(&mut self, email: Email) {
        let before = self.look.email.replace(email);
        // A removal settingsd shows: Email forgets the account's password.
        if let Some((id, name)) = self.email.removed.take()
            && self
                .look
                .email
                .as_ref()
                .is_some_and(|e| e.account(&id).is_none())
        {
            self.forget_removed(id, name);
        }
        let email = self.look.email.as_ref().expect("just read");
        // A sent Add not in the read did not land (or another writer was
        // first): its typed form stays.
        if let Some(id) = self.email.added.take()
            && let Some(account) = email.account(&id)
        {
            self.ui.email_account = Some(id);
            self.email.form = Some(Form::of(account));
            self.email.password = Secret::default();
            return;
        }
        let Some(form) = &self.email.form else {
            return;
        };
        let Some(id) = form.id.clone() else {
            return;
        };
        match email.account(&id) {
            None => {
                self.email.form = None;
                self.email.password = Secret::default();
                self.ui.email_account = None;
            }
            Some(account) => {
                let saved = before.as_ref().and_then(|b| b.account(&id));
                if !form.edited(saved) || !form.edited(Some(account)) {
                    self.email.form = Some(Form::of(account));
                }
            }
        }
    }

    // ---- the account list -------------------------------------------------

    /// Select `id` and show its form. Typing in another account's form is
    /// dropped, as is a typed password.
    pub fn select_email_account(&mut self, id: &str) {
        if self.ui.dialog.is_some() || self.quitting {
            return;
        }
        let Some(account) = self.email().and_then(|e| e.account(id)).cloned() else {
            return;
        };
        if self.ui.email_account.as_deref() == Some(id) && self.email.form.is_some() {
            return;
        }
        self.ui.email_account = Some(account.id.clone());
        self.email.form = Some(Form::of(&account));
        self.email.password = Secret::default();
    }

    pub fn can_add_email_account(&self) -> bool {
        self.look.email.is_some() && self.ui.dialog.is_none() && !self.quitting
    }

    /// A blank form for a new account; Save adds it.
    pub fn new_email_account(&mut self) {
        if self.can_add_email_account() {
            self.ui.email_account = None;
            self.email.form = Some(Form::blank());
            self.email.password = Secret::default();
        }
    }

    /// The identity list the editor shows for the selected account: the
    /// draft when it is for that account, else the account's senders as saved.
    pub fn identity_draft(&self) -> IdentityDraft {
        let account = self.ui.email_account.clone().unwrap_or_default();
        match &self.email.identities {
            Some(draft) if draft.account == account => draft.clone(),
            _ => IdentityDraft {
                account,
                rows: self
                    .email_account()
                    .map(|a| a.senders())
                    .unwrap_or_default(),
                selected: 0,
                error: None,
            },
        }
    }

    /// The draft for the selected account, started from its saved senders
    /// when there is none yet.
    fn identity_draft_mut(&mut self) -> Option<&mut IdentityDraft> {
        let account = self.ui.email_account.clone()?;
        let stale = self
            .email
            .identities
            .as_ref()
            .is_none_or(|draft| draft.account != account);
        if stale {
            let rows = self.email_account()?.senders();
            self.email.identities = Some(IdentityDraft {
                account,
                rows,
                selected: 0,
                error: None,
            });
        }
        self.email.identities.as_mut()
    }

    /// One text field of the selected identity, as typed.
    pub fn identity_edit(&mut self, field: IdentityField, value: String) {
        let Some(draft) = self.identity_draft_mut() else {
            return;
        };
        if let Some(row) = draft.rows.get_mut(draft.selected) {
            match field {
                IdentityField::Name => row.name = value,
                IdentityField::Address => row.address = value,
                IdentityField::Signature => row.signature = value,
                IdentityField::ReplyTo => row.reply_to = Some(value).filter(|v| !v.is_empty()),
            }
            draft.error = None;
        }
    }

    /// Select identity `index` of the draft.
    pub fn identity_select(&mut self, index: usize) {
        if let Some(draft) = self.identity_draft_mut()
            && index < draft.rows.len()
        {
            draft.selected = index;
        }
    }

    /// Add an identity with a fresh id, and select it.
    pub fn identity_add(&mut self) {
        let Some(draft) = self.identity_draft_mut() else {
            return;
        };
        let mut n = draft.rows.len() + 1;
        let id = loop {
            let candidate = format!("identity-{n}");
            if !draft.rows.iter().any(|r| r.id == candidate) {
                break candidate;
            }
            n += 1;
        };
        draft.rows.push(settings::email::Identity {
            id,
            name: String::new(),
            address: String::new(),
            signature: String::new(),
            reply_to: None,
        });
        draft.selected = draft.rows.len() - 1;
        draft.error = None;
    }

    /// Remove the selected identity of the draft.
    pub fn identity_remove(&mut self) {
        let Some(draft) = self.identity_draft_mut() else {
            return;
        };
        if draft.rows.is_empty() {
            return;
        }
        draft.rows.remove(draft.selected);
        draft.selected = draft.selected.min(draft.rows.len().saturating_sub(1));
        draft.error = None;
    }

    /// Throw the identity draft away.
    pub fn identity_revert(&mut self) {
        self.email.identities = None;
    }

    /// Send the draft as the account's identity list. Settings decides; a
    /// refused list keeps the draft with the reason. A saved list clears it.
    pub fn identity_save(&mut self) {
        let Some(draft) = self.email.identities.clone() else {
            return;
        };
        let change = Change::Identities(draft.account.clone(), draft.rows.clone());
        match self.change_email(change) {
            Ok(_) | Err(Refusal::Unchanged) => self.email.identities = None,
            Err(refusal) => {
                if let Some(draft) = &mut self.email.identities {
                    draft.error = Some(refusal);
                }
            }
        }
    }

    /// One form field as typed.
    pub fn edit_email_field(&mut self, field: Field, text: String) {
        if let Some(form) = &mut self.email.form {
            *form.slot(field) = text;
            form.errors.remove(&field);
        }
    }

    pub fn can_save_email_account(&self) -> bool {
        self.can_edit_email()
            && self
                .email
                .form
                .as_ref()
                .is_some_and(|f| f.edited(self.email_account()) || f.id.is_none())
    }

    /// Save the form: Add for a new account, else Update.
    pub fn save_email_account(&mut self) {
        if !self.can_save_email_account() {
            return;
        }
        let Some(form) = self.email.form.clone() else {
            return;
        };
        let change = match &form.id {
            None => Change::Add(form.patch()),
            Some(id) => Change::Update(id.clone(), form.patch()),
        };
        let _ = self.change_email(change);
    }

    pub fn can_revert_email_account(&self) -> bool {
        self.ui.dialog.is_none()
            && self
                .email
                .form
                .as_ref()
                .is_some_and(|f| f.edited(self.email_account()))
    }

    /// Drop the typing: the form shows settingsd's account again (a new
    /// account's form closes).
    pub fn revert_email_account(&mut self) {
        if !self.can_revert_email_account() {
            return;
        }
        self.email.form = self.email_account().map(Form::of);
    }

    /// An account is selected and changes can go out.
    fn selected_editable(&self) -> Option<&AccountEntry> {
        self.can_edit_email()
            .then(|| self.email_account())
            .flatten()
    }

    pub fn can_remove_email_account(&self) -> bool {
        self.selected_editable().is_some() && self.email.task.is_none()
    }

    /// Remove account `id`: through Email when it may be running, so its
    /// guard holds (it refuses while the account migrates, and forgets the
    /// password itself); straight to settingsd, with a best-effort forget,
    /// when Email is known to be away. `reply` is the `prefs.email.set`
    /// that asked: answered now on the direct path, else once Email has.
    pub(crate) fn remove_account(&mut self, id: String, reply: Option<u64>) -> Result<(), Refusal> {
        let refuse = |e: &mut Engine, code: &str, key: &'static str| -> Result<(), Refusal> {
            if let Some(reply) = reply {
                let message = e.label(key, &[]);
                e.effects.push(Effect::Reply {
                    id: reply,
                    rc: 10,
                    body: json!({"error_code":code,"message":message}),
                });
            }
            Err(Refusal::Key(key))
        };
        if self.look.email.is_none() {
            return refuse(self, "NOT_READY", "email-not-ready");
        }
        if !self.can_edit_email() || self.email.task.is_some() {
            return refuse(self, "BUSY", "busy");
        }
        if self.email().is_none_or(|e| e.account(&id).is_none()) {
            self.email_refused(&Change::Remove(id), &Refusal::Key("email-unknown-account"));
            return refuse(self, "ARGUMENT", "email-unknown-account");
        }
        if self.email.service == Service::Absent {
            return self.remove_directly(id, reply);
        }
        let name = self.account_name(&id);
        self.begin_action(Action::Remove(id), reply, false, name);
        Ok(())
    }

    /// The settingsd removal (Email away, or too old to remove): one fenced
    /// apply, then a best-effort forget once the read shows it gone.
    fn remove_directly(&mut self, id: String, reply: Option<u64>) -> Result<(), Refusal> {
        let result = self.change_email(Change::Remove(id)).map(|_| ());
        if let Some(reply) = reply {
            match &result {
                Ok(()) => {
                    let mut body = self.email_view();
                    body["applying"] = json!(true);
                    self.effects.push(Effect::Reply {
                        id: reply,
                        rc: 0,
                        body,
                    });
                }
                Err(refusal) => {
                    let message = match refusal {
                        Refusal::Key(key) => self.label(key, &[]),
                        Refusal::Settings(message) => message.clone(),
                        _ => self.status.clone(),
                    };
                    self.effects.push(Effect::Reply {
                        id: reply,
                        rc: 10,
                        body: json!({"error_code":"ARGUMENT","message":message}),
                    });
                }
            }
        }
        result
    }

    /// Ask before removing the selected account.
    pub fn remove_email_account(&mut self) {
        if let Some(id) = self.selected_editable().map(|a| a.id.clone()) {
            self.open(Dialog::RemoveAccount(id));
        }
    }

    fn selected_index(&self) -> Option<(usize, usize)> {
        let account = self.selected_editable()?;
        let accounts = &self.email()?.accounts;
        Some((
            accounts.iter().position(|a| a.id == account.id)?,
            accounts.len(),
        ))
    }

    pub fn can_move_email_account_up(&self) -> bool {
        self.selected_index().is_some_and(|(at, _)| at > 0)
    }

    pub fn can_move_email_account_down(&self) -> bool {
        self.selected_index().is_some_and(|(at, n)| at + 1 < n)
    }

    pub fn move_email_account_up(&mut self) {
        self.move_selected(Direction::Up);
    }

    pub fn move_email_account_down(&mut self) {
        self.move_selected(Direction::Down);
    }

    fn move_selected(&mut self, direction: Direction) {
        if let Some(id) = self.ui.email_account.clone()
            && self.selected_editable().is_some()
        {
            let _ = self.change_email(Change::Move(id, direction));
        }
    }

    pub fn can_default_email_account(&self) -> bool {
        self.selected_editable().is_some_and(|a| {
            self.email()
                .is_some_and(|e| e.default_account.as_deref() != Some(a.id.as_str()))
        })
    }

    /// The selected account sends new mail by default.
    pub fn default_email_account(&mut self) {
        if self.can_default_email_account()
            && let Some(id) = self.ui.email_account.clone()
        {
            let _ = self.change_email(Change::Default(Some(id)));
        }
    }

    pub fn can_enable_email_account(&self) -> bool {
        self.selected_editable().is_some()
    }

    /// Enable the selected account, or disable it.
    pub fn toggle_email_account(&mut self) {
        if let Some(account) = self.selected_editable().cloned() {
            let _ = self.change_email(Change::Enable(account.id, !account.enabled));
        }
    }

    pub fn can_reset_email(&self) -> bool {
        self.can_edit_email()
    }

    /// Ask before resetting every email setting.
    pub fn reset_email(&mut self) {
        if self.can_reset_email() {
            self.open(Dialog::ResetEmail);
        }
    }

    /// The remove or reset confirmation's confirm button.
    pub fn confirm_email(&mut self) {
        match self.ui.dialog.clone() {
            Some(Dialog::RemoveAccount(id)) => {
                self.ui.dialog = None;
                let _ = self.remove_account(id, None);
            }
            Some(Dialog::ForgetPassword(id)) => {
                self.ui.dialog = None;
                if self.can_connect_email_account() {
                    self.start_email_action(Action::Forget(id), None);
                }
            }
            Some(Dialog::ResetEmail) => {
                self.ui.dialog = None;
                if self.can_edit_email() {
                    self.email.added = None;
                    self.send_email(Edit::Reset);
                }
            }
            _ => {}
        }
        self.pump_look();
    }

    // ---- passwords and connections ---------------------------------------

    /// The password field as typed.
    pub fn edit_email_password(&mut self, password: Secret) {
        self.email.password = password;
    }

    pub fn can_set_email_password(&self) -> bool {
        self.email_account().is_some()
            && !self.email.password.is_empty()
            && self.email.task.is_none()
            && self.connected
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    /// Send the typed password to Email for the selected account. The field
    /// is emptied at once; it comes back only if Email refuses it.
    pub fn set_email_password(&mut self) {
        if !self.can_set_email_password() {
            return;
        }
        let Some(id) = self.ui.email_account.clone() else {
            return;
        };
        let password = std::mem::take(&mut self.email.password);
        self.start_email_action(Action::Password(id, password), None);
    }

    pub fn can_connect_email_account(&self) -> bool {
        self.email_account().is_some()
            && self.email.task.is_none()
            && self.connected
            && self.ui.dialog.is_none()
            && !self.quitting
    }

    /// Ask Email to connect the selected account now.
    pub fn connect_email_account(&mut self) {
        if self.can_connect_email_account()
            && let Some(id) = self.ui.email_account.clone()
        {
            self.start_email_action(Action::Connect(id), None);
        }
    }

    fn account_name(&self, id: &str) -> String {
        self.email()
            .and_then(|e| e.account(id))
            .map_or_else(|| id.to_owned(), |a| a.name.clone())
    }

    pub fn can_forget_email_password(&self) -> bool {
        self.can_connect_email_account()
    }

    /// Ask before Email forgets the selected account's saved password.
    pub fn forget_email_password(&mut self) {
        if self.can_forget_email_password()
            && let Some(id) = self.ui.email_account.clone()
        {
            self.open(Dialog::ForgetPassword(id));
        }
    }

    /// settingsd shows account `id` removed: ask an Email already running to
    /// forget its password, best effort. Email is not launched for this; if
    /// it is not running, or refuses, a note says the password may stay.
    fn forget_removed(&mut self, id: String, name: String) {
        if self.email.service == Service::Absent {
            let message = self.label("email-not-running", &[]);
            let text = self.label(
                "email-forget-after-remove-failed",
                &[("account", &name), ("message", &message)],
            );
            self.email.outcome = Some((text.clone(), true));
            self.status = text;
            self.failed = true;
        } else if self.email.task.is_some() {
            self.email.forgets.push((id, name));
        } else {
            self.begin_action(Action::Forget(id), None, false, name);
        }
    }

    fn start_email_action(&mut self, action: Action, reply: Option<u64>) {
        let name = self.account_name(action.id());
        self.begin_action(action, reply, true, name);
    }

    fn begin_action(&mut self, action: Action, reply: Option<u64>, may_launch: bool, name: String) {
        let key = match action {
            Action::Password(..) => "email-password-sending",
            Action::Forget(_) => "email-password-forgetting",
            Action::Remove(_) => "email-removing",
            Action::Connect(_) => "email-connecting",
        };
        self.status = self.label(key, &[("account", &name)]);
        self.failed = false;
        self.email.outcome = None;
        let ticket = self.next();
        self.email.task = Some(Task {
            action,
            stage: Stage::Sending,
            ticket,
            launched: false,
            waits: 0,
            reply,
            may_launch,
            name,
        });
        self.send_action();
    }

    /// Send the task's call to Email under a fresh ticket, so an earlier
    /// wait's late firing is ignored.
    fn send_action(&mut self) {
        if self.email.task.is_none() {
            return;
        }
        let ticket = self.next();
        let task = self.email.task.as_mut().expect("a task");
        task.ticket = ticket;
        task.stage = Stage::Sending;
        let secret = match &task.action {
            Action::Password(_, password) => Some(password.clone()),
            Action::Forget(_) | Action::Connect(_) | Action::Remove(_) => None,
        };
        let verb = task.action.verb();
        // The one empty password sent: a forget, never a typed Set password.
        let body = match &task.action {
            Action::Forget(id) => json!({"id":id,"password":""}),
            action => json!({"id":action.id()}),
        };
        self.effects.push(Effect::Mail {
            ticket,
            service: EMAIL,
            verb,
            body,
            secret,
        });
    }

    /// Ask Email whether each account has a password. Nothing is asked
    /// while a question is in flight, nor before settingsd is read.
    pub fn query_email_accounts(&mut self) {
        if self.email.accounts_job.is_some() || !self.connected || self.quitting {
            return;
        }
        let ticket = self.next();
        self.email.accounts_job = Some(ticket);
        self.effects.push(Effect::Mail {
            ticket,
            service: EMAIL,
            verb: "email.accounts",
            body: json!({}),
            secret: None,
        });
    }

    /// The set of Bus services changed: Email may have come or gone, and a
    /// task waiting for it to register tries now.
    pub(crate) fn email_services_changed(&mut self) {
        if self.section() == Section::Email {
            self.query_email_accounts();
        }
        if self
            .email
            .task
            .as_ref()
            .is_some_and(|t| t.stage == Stage::Waiting)
        {
            self.send_action();
        }
    }

    /// A call to Email (or to apps) finished.
    pub fn mailed(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        if self.email.accounts_job == Some(ticket) {
            self.email.accounts_job = None;
            self.accounts_answered(crate::appearance::answer(result));
        } else if let Some(task) = self.email.task.clone()
            && task.ticket == ticket
        {
            let answer = crate::appearance::answer(result);
            match task.stage {
                Stage::Sending => self.action_answered(task, answer),
                Stage::Launching => self.launch_answered(task, answer),
                Stage::Waiting => {}
            }
        }
        if self.quitting && !self.busy() {
            self.close_now();
        }
    }

    fn accounts_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(value) => match passwords(&value) {
                Some(map) => {
                    self.email.service = Service::Running;
                    self.email.passwords = map;
                }
                None => {
                    self.email.service = Service::Unsupported;
                    self.email.passwords.clear();
                }
            },
            Answer::Missing => {
                self.email.service = Service::Absent;
                self.email.passwords.clear();
            }
            Answer::Refused(value) if unknown_verb(&value) => {
                self.email.service = Service::Unsupported;
                self.email.passwords.clear();
            }
            // Busy or lost: what was known stays.
            Answer::Refused(_) | Answer::NotSent(_) | Answer::Lost(_) => {}
        }
    }

    fn action_answered(&mut self, task: Task, answer: Answer) {
        let name = task.name.clone();
        // A forget after a removal: best effort, on an Email already up.
        if matches!(task.action, Action::Forget(_))
            && !task.may_launch
            && !matches!(answer, Answer::Ok(_))
        {
            return self.follow_up_failed(task, answer);
        }
        if let Action::Remove(id) = &task.action {
            return self.remove_answered(id.clone(), task, answer);
        }
        match answer {
            Answer::Ok(value) => {
                let key = match task.action {
                    Action::Password(..) => "email-password-set",
                    Action::Forget(_) => "email-password-forgotten",
                    Action::Remove(_) => "email-removed",
                    Action::Connect(_) => "email-connect-asked",
                };
                let detail = value["message"]
                    .as_str()
                    .or_else(|| value["status"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                let mut text = self.label(key, &[("account", &name)]);
                if !detail.is_empty() {
                    text = format!("{text} ({detail})");
                }
                let body = json!({"done":true,"account":task_id(&task)});
                self.end_action(task, Ok(text), body);
                self.email.service = Service::Running;
                self.query_email_accounts();
            }
            Answer::Missing if !task.launched && task.may_launch => self.launch_email(task),
            Answer::Missing => self.wait_for_email(task),
            Answer::Refused(value) if unknown_verb(&value) => {
                let text = self.label("email-unsupported", &[]);
                self.end_action(task, Err(("UNSUPPORTED", text)), Value::Null);
            }
            Answer::Refused(value) => {
                let message = refusal(&value);
                let text = self.label(
                    "email-action-failed",
                    &[("account", &name), ("message", &message)],
                );
                self.end_action(task, Err(("REFUSED", text)), Value::Null);
            }
            Answer::NotSent(message) => {
                let text = self.label(
                    "email-action-failed",
                    &[("account", &name), ("message", &message)],
                );
                self.end_action(task, Err(("NOT_SENT", text)), Value::Null);
            }
            // Setting the same password again, or asking to connect again,
            // is harmless: the person may simply try again.
            Answer::Lost(message) => {
                let text = self.label(
                    "email-action-lost",
                    &[("account", &name), ("message", &message)],
                );
                self.end_action(task, Err(("OUTCOME_UNKNOWN", text)), Value::Null);
            }
        }
    }

    /// Email answered a Remove. Done, settingsd is read again (Email has
    /// changed it); refused (`MIGRATION_PENDING` among them), Email's
    /// answer is shown and returned as it gave it. Email gone, or too old
    /// to remove accounts: the settingsd removal instead.
    fn remove_answered(&mut self, id: String, task: Task, answer: Answer) {
        let name = task.name.clone();
        let reply = task.reply;
        match answer {
            Answer::Ok(_) => {
                let text = self.label("email-removed", &[("account", &name)]);
                let body = json!({"removed":id,"via":"email"});
                self.end_action(task, Ok(text), body);
                self.read_look();
            }
            Answer::Missing => {
                self.email.task = None;
                self.email.service = Service::Absent;
                let _ = self.remove_directly(id, reply);
            }
            Answer::Refused(value) if unknown_verb(&value) => {
                self.email.task = None;
                let _ = self.remove_directly(id, reply);
            }
            Answer::Refused(value) => {
                let message = match (&value["error_code"], &value["message"]) {
                    (Value::String(code), Value::String(message)) => {
                        redact_text(&format!("{code}: {message}"))
                    }
                    _ => refusal(&value),
                };
                let text = self.label(
                    "email-action-failed",
                    &[("account", &name), ("message", &message)],
                );
                // The Bus caller gets Email's refusal as Email gave it.
                let task = Task {
                    reply: None,
                    ..task
                };
                self.end_action(task, Err(("REFUSED", text)), Value::Null);
                if let Some(reply) = reply {
                    let mut body = json!({"error_code":value["error_code"],"message":message});
                    if body["error_code"].is_null() {
                        body["error_code"] = json!("REFUSED");
                    }
                    self.effects.push(Effect::Reply {
                        id: reply,
                        rc: 10,
                        body,
                    });
                }
            }
            // Unknown: the next read shows whether it went.
            Answer::Lost(message) => {
                let text = self.label(
                    "email-action-lost",
                    &[("account", &name), ("message", &message)],
                );
                self.end_action(task, Err(("OUTCOME_UNKNOWN", text)), Value::Null);
                self.read_look();
            }
            Answer::NotSent(message) => {
                let text = self.label(
                    "email-action-failed",
                    &[("account", &name), ("message", &message)],
                );
                self.end_action(task, Err(("NOT_SENT", text)), Value::Null);
            }
        }
    }

    /// A forget after a removal was not done. Email not knowing the
    /// account means it held nothing to forget; anything else is a note
    /// that the password may stay with Email.
    fn follow_up_failed(&mut self, task: Task, answer: Answer) {
        let name = task.name.clone();
        let message = match answer {
            Answer::Refused(value) if value["error_code"] == "UNKNOWN_ACCOUNT" => {
                let text = self.label("email-password-forgotten", &[("account", &name)]);
                return self.end_action(task, Ok(text), Value::Null);
            }
            Answer::Refused(value) if unknown_verb(&value) => self.label("email-unsupported", &[]),
            Answer::Refused(value) => refusal(&value),
            Answer::Missing => self.label("email-not-running", &[]),
            Answer::NotSent(message) | Answer::Lost(message) => message,
            Answer::Ok(_) => String::new(),
        };
        let text = self.label(
            "email-forget-after-remove-failed",
            &[("account", &name), ("message", &message)],
        );
        self.end_action(task, Err(("NOT_FORGOTTEN", text)), Value::Null);
    }

    /// Email is not on the Bus: launch it, once per task.
    fn launch_email(&mut self, mut task: Task) {
        self.status = self.label("email-launching", &[]);
        self.failed = false;
        let ticket = self.next();
        task.ticket = ticket;
        task.stage = Stage::Launching;
        task.launched = true;
        self.email.task = Some(task);
        self.effects.push(Effect::Mail {
            ticket,
            service: APPS,
            verb: "apps.launch",
            body: json!({"id":EMAIL_APP_ID}),
            secret: None,
        });
    }

    fn launch_answered(&mut self, task: Task, answer: Answer) {
        match answer {
            Answer::Ok(_) => self.wait_for_email(task),
            Answer::Missing => {
                let text = self.label("email-no-launcher", &[]);
                self.end_action(task, Err(("NOT_RUNNING", text)), Value::Null);
            }
            Answer::Refused(value) => {
                let message = refusal(&value);
                let text = self.label("email-launch-failed", &[("message", &message)]);
                self.end_action(task, Err(("NOT_RUNNING", text)), Value::Null);
            }
            // A lost launch may have started Email: wait for it all the same.
            Answer::Lost(_) => self.wait_for_email(task),
            Answer::NotSent(message) => {
                let text = self.label("email-launch-failed", &[("message", &message)]);
                self.end_action(task, Err(("NOT_RUNNING", text)), Value::Null);
            }
        }
    }

    /// Wait for Email to register, or give up once the waits are spent.
    fn wait_for_email(&mut self, mut task: Task) {
        let Some(&after_ms) = LAUNCH_WAITS_MS.get(task.waits) else {
            let text = self.label("email-did-not-start", &[]);
            self.end_action(task, Err(("NOT_RUNNING", text)), Value::Null);
            return;
        };
        self.status = self.label("email-waiting", &[]);
        self.failed = false;
        let ticket = self.next();
        task.ticket = ticket;
        task.stage = Stage::Waiting;
        task.waits += 1;
        self.email.task = Some(task);
        self.effects.push(Effect::Wait { ticket, after_ms });
    }

    /// A wait asked for by [`Effect::Wait`] is over: try Email again.
    pub fn waited(&mut self, ticket: u64) {
        if self
            .email
            .task
            .as_ref()
            .is_some_and(|t| t.ticket == ticket && t.stage == Stage::Waiting)
        {
            self.send_action();
        }
    }

    /// The task is over: say how, answer the Bus command that asked, and
    /// give a refused password back to the field (a sent one is dropped).
    fn end_action(
        &mut self,
        task: Task,
        outcome: Result<String, (&'static str, String)>,
        body: Value,
    ) {
        self.email.task = None;
        let failed = outcome.is_err();
        let text = match &outcome {
            Ok(text) | Err((_, text)) => text.clone(),
        };
        self.status = text.clone();
        self.failed = failed;
        self.email.outcome = Some((text.clone(), failed));
        if let (true, Action::Password(id, password)) = (failed, &task.action)
            && task.reply.is_none()
            && self.ui.email_account.as_deref() == Some(id.as_str())
            && self.email.password.is_empty()
        {
            self.email.password = password.clone();
        }
        if let Some(id) = task.reply {
            match outcome {
                Ok(_) => self.effects.push(Effect::Reply { id, rc: 0, body }),
                Err((code, message)) => self.effects.push(Effect::Reply {
                    id,
                    rc: 10,
                    body: json!({"error_code":code,"message":message}),
                }),
            }
        }
        // A removal's forget that waited for this action goes now.
        if let Some((id, name)) = self.email.forgets.pop() {
            self.begin_action(Action::Forget(id), None, false, name);
        }
    }

    // ---- the Bus surface ---------------------------------------------------

    /// `prefs.email`: the email settings as settingsd last held them, with
    /// `has_password` per account when Email reported it. Never a secret.
    pub fn email_view(&self) -> Value {
        let email = self.email();
        let accounts: Vec<Value> = email
            .map(|e| {
                e.accounts
                    .iter()
                    .map(|a| {
                        let mut row = json!(a);
                        row["url"] = json!(redact_url(&a.url));
                        row["has_password"] = json!(self.email.passwords.get(&a.id));
                        row["default"] = json!(e.default_account.as_deref() == Some(a.id.as_str()));
                        row
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mail = email.map(|e| &e.mail);
        json!({"settings":self.look.settings,
            "revision":self.look.revision().map(|r| r.to_string()),
            "read":email.is_some(),
            "accounts":accounts,
            "default_account":email.and_then(|e| e.default_account.clone()),
            "mail":mail.map(|m| json!({"page_size":m.page_size,"mark_read_on_open":m.mark_read_on_open,"confirm_delete":m.confirm_delete,"quote_on_reply":m.quote_on_reply,"reply_position":m.reply_position})),
            "email_service":self.email.service,
            "selected":self.ui.email_account,
            "form":self.email.form,
            "password_typed":!self.email.password.is_empty(),
            "task":self.email.task.as_ref().map(|t| json!({"action":t.action.kind(),"account":task_id(t),"stage":t.stage})),
            "outcome":self.email.outcome.as_ref().map(|(text, failed)| json!({"text":text,"failed":failed})),
            "can_edit":self.can_edit_email()})
    }

    /// `prefs.email.set`: one change, through the same paths as the window.
    pub(crate) fn set_email(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        let change = email_change(args).map_err(|m| ("ARGUMENT", m))?;
        if self.look.email.is_none() {
            return Err(("NOT_READY", self.label("email-not-ready", &[])));
        }
        if !self.can_edit_email() {
            return Err(("BUSY", self.label("busy", &[])));
        }
        match self.change_email(change) {
            Ok(planned) => {
                let mut body = self.email_view();
                body["applying"] = json!(true);
                body["id"] = json!(planned.added);
                Ok(body)
            }
            Err(Refusal::Unchanged) => {
                let mut body = self.email_view();
                body["applying"] = json!(false);
                Ok(body)
            }
            Err(Refusal::Fields(fields)) => {
                let text: Vec<String> = fields
                    .iter()
                    .map(|(f, key)| format!("{}: {}", f.name(), self.label(key, &[])))
                    .collect();
                Err(("ARGUMENT", text.join("; ")))
            }
            Err(Refusal::Key(key)) => Err(("ARGUMENT", self.label(key, &[]))),
            Err(Refusal::Settings(message)) => Err(("ARGUMENT", message)),
        }
    }

    /// `prefs.email.password`: hand `password` for account `id` to Email,
    /// answered once Email has (launching it first when it is absent).
    pub(crate) fn email_password(&mut self, id: u64, args: &Value) {
        let argument = |e: &mut Engine, message: &str| {
            e.effects.push(Effect::Reply {
                id,
                rc: 10,
                body: json!({"error_code":"ARGUMENT","message":message}),
            });
        };
        let Some(account) = args.get("id").and_then(Value::as_str) else {
            return argument(self, "id must be an account id");
        };
        // `forget: true` deletes the saved password: the one empty password
        // sent. Any id is taken, so a removed account's can be forgotten.
        let forget = match args.get("forget") {
            None | Some(Value::Bool(false)) => false,
            Some(Value::Bool(true)) => true,
            Some(_) => return argument(self, "forget must be true or false"),
        };
        let password = match (forget, args.get("password")) {
            (true, None) => None,
            (true, Some(_)) => return argument(self, "give password or forget, not both"),
            (false, Some(Value::String(password))) => Some(Secret::new(password.as_str())),
            (false, _) => return argument(self, "password must be a string"),
        };
        let refuse = |e: &mut Engine, code: &str, key: &str| {
            let message = e.label(key, &[]);
            e.effects.push(Effect::Reply {
                id,
                rc: 10,
                body: json!({"error_code":code,"message":message}),
            });
        };
        if password.as_ref().is_some_and(Secret::is_empty) {
            return refuse(self, "ARGUMENT", "email-password-empty");
        }
        if !settings::email::valid_id(account)
            || (password.is_some() && self.email().is_some_and(|e| e.account(account).is_none()))
        {
            return refuse(self, "ARGUMENT", "email-unknown-account");
        }
        if self.email.task.is_some() || !self.connected || self.quitting {
            return refuse(self, "BUSY", "busy");
        }
        let action = match password {
            Some(password) => Action::Password(account.to_owned(), password),
            None => Action::Forget(account.to_owned()),
        };
        self.start_email_action(action, Some(id));
    }
}

fn task_id(task: &Task) -> &str {
    task.action.id()
}

/// The change a `prefs.email.set` object asks for: `{op, …}` for an
/// account, or `{path, value}` for a mail option.
pub fn email_change(args: &Value) -> Result<Change, String> {
    let id = || {
        args.get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "id must be an account id".to_owned())
    };
    if let Some(path) = args.get("path") {
        let value = args.get("value").ok_or("value is required with path")?;
        return match path.as_str() {
            Some("email.mail.page_size") => value
                .as_f64()
                .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= f64::from(u32::MAX))
                .map(|v| Change::PageSize(v as u32))
                .ok_or_else(|| "page_size must be a whole number".into()),
            Some("email.mail.mark_read_on_open") => value
                .as_bool()
                .map(Change::MarkReadOnOpen)
                .ok_or_else(|| "mark_read_on_open must be true or false".into()),
            Some("email.mail.confirm_delete") => value
                .as_bool()
                .map(Change::ConfirmDelete)
                .ok_or_else(|| "confirm_delete must be true or false".into()),
            Some("email.mail.quote_on_reply") => value
                .as_bool()
                .map(Change::QuoteOnReply)
                .ok_or_else(|| "quote_on_reply must be true or false".into()),
            Some("email.mail.reply_position") => match value.as_str() {
                Some("above") => Ok(Change::ReplyPosition(settings::email::ReplyPosition::Above)),
                Some("below") => Ok(Change::ReplyPosition(settings::email::ReplyPosition::Below)),
                _ => Err("reply_position must be \"above\" or \"below\"".into()),
            },
            _ => Err("path must be email.mail.page_size, email.mail.mark_read_on_open, email.mail.confirm_delete, email.mail.quote_on_reply or email.mail.reply_position".into()),
        };
    }
    match args.get("op").and_then(Value::as_str) {
        Some("add") => {
            let patch = Patch::from_args(args)?;
            if patch.name.is_none() || patch.url.is_none() || patch.email.is_none() {
                return Err("add needs name, url and email".into());
            }
            Ok(Change::Add(patch))
        }
        Some("update") => Ok(Change::Update(id()?, Patch::from_args(args)?)),
        Some("remove") => Ok(Change::Remove(id()?)),
        Some("move") => match args.get("direction").and_then(Value::as_str) {
            Some("up") => Ok(Change::Move(id()?, Direction::Up)),
            Some("down") => Ok(Change::Move(id()?, Direction::Down)),
            _ => Err("direction must be \"up\" or \"down\"".into()),
        },
        Some("default") => match args.get("id") {
            Some(Value::Null) | None => Ok(Change::Default(None)),
            Some(Value::String(id)) => Ok(Change::Default(Some(id.clone()))),
            Some(_) => Err("id must be an account id or null".into()),
        },
        Some("identities") => {
            let list = args.get("identities").ok_or("identities must be a list")?;
            let identities: Vec<settings::email::Identity> = serde_json::from_value(list.clone())
                .map_err(|e| format!("identities: {e}"))?;
            Ok(Change::Identities(id()?, identities))
        }
        Some("enable") => match args.get("enabled") {
            Some(Value::Bool(on)) => Ok(Change::Enable(id()?, *on)),
            _ => Err("enabled must be true or false".into()),
        },
        _ => Err(
            "op must be add, update, remove, move, default, enable or identities (or give path and value)"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Effect;

    #[test]
    fn identities_are_planned_as_the_whole_list_and_checked() {
        use settings::email::Identity;
        let work = Identity {
            id: "work".into(),
            name: "Ann Work".into(),
            address: "ann@work.example.org".into(),
            signature: String::new(),
            reply_to: None,
        };
        let planned = applied(
            plan(
                &two(),
                &Change::Identities("a".into(), vec![work.clone()]),
                mint,
            )
            .unwrap(),
        );
        let accounts = planned
            .get("email.accounts")
            .expect("the list is written whole");
        assert_eq!(accounts[0]["identities"][0]["id"], "work");
        let bad = Identity {
            address: "not-an-address".into(),
            ..work
        };
        assert!(matches!(
            plan(&two(), &Change::Identities("a".into(), vec![bad]), mint),
            Err(Refusal::Settings(_))
        ));
        assert!(matches!(
            plan(&two(), &Change::Identities("zz".into(), vec![]), mint),
            Err(Refusal::Key(_))
        ));
    }

    #[test]
    fn the_identities_op_takes_a_list_over_the_bus() {
        let args = json!({"op": "identities", "id": "a", "identities": [
            {"id": "work", "name": "Ann", "address": "ann@work.example.org"}
        ]});
        let Ok(Change::Identities(id, rows)) = email_change(&args) else {
            panic!("the op parses");
        };
        assert_eq!(id, "a");
        assert_eq!(rows[0].id, "work");
        let text = json!({"op": "identities", "id": "a", "identities": "work"});
        assert!(email_change(&text).is_err());
    }

    fn account(id: &str, name: &str) -> AccountEntry {
        AccountEntry {
            id: id.into(),
            name: name.into(),
            url: "https://mail.example.org".into(),
            email: format!("{id}@example.org"),
            ca_path: None,
            enabled: true,
            signature: String::new(),
            identities: Vec::new(),
        }
    }

    fn two() -> Email {
        Email {
            accounts: vec![account("a", "Work"), account("b", "Home")],
            default_account: Some("a".into()),
            ..Default::default()
        }
    }

    fn mint(_: &[AccountEntry]) -> String {
        "acct-new".into()
    }

    fn applied(planned: Planned) -> BTreeMap<String, Value> {
        match planned.edit {
            Edit::Apply(changes) => changes,
            Edit::Reset => panic!("not a reset"),
        }
    }

    #[test]
    fn add_mints_an_id_and_sends_the_whole_list() {
        let patch = Patch {
            name: Some(" Gold Coast ".into()),
            url: Some("https://mail.example.com".into()),
            email: Some("admin@example.com".into()),
            ca_path: Some(Some("  ".into())),
            ..Default::default()
        };
        let planned = plan(&two(), &Change::Add(patch), mint).unwrap();
        assert_eq!(planned.added.as_deref(), Some("acct-new"));
        let changes = applied(planned);
        assert_eq!(changes.len(), 1, "{changes:?}");
        let list = changes["email.accounts"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[2]["id"], "acct-new");
        assert_eq!(list[2]["name"], "Gold Coast", "trimmed");
        assert_eq!(list[2]["ca_path"], Value::Null, "blank is the system roots");
        assert_eq!(list[2]["enabled"], true);
        // The real minter gives a valid, unused id.
        let id = mint_account_id(&two().accounts);
        assert!(settings::email::valid_id(&id), "{id}");
    }

    #[test]
    fn every_bad_field_is_named_and_nothing_is_sent() {
        let patch = Patch {
            name: Some(String::new()),
            url: Some("http://mail.example.org/".into()),
            email: Some("admin".into()),
            ca_path: Some(Some("ca.pem".into())),
            signature: Some("bell\u{7}".into()),
            enabled: None,
        };
        let Err(Refusal::Fields(fields)) = plan(&two(), &Change::Add(patch), mint) else {
            panic!("refused by field");
        };
        assert_eq!(fields.keys().copied().collect::<Vec<_>>(), Field::ALL);
        assert_eq!(fields[&Field::Url], "email-url-refused");
    }

    #[test]
    fn removing_the_default_clears_it_in_the_same_batch() {
        let changes = applied(plan(&two(), &Change::Remove("a".into()), mint).unwrap());
        assert_eq!(changes["email.accounts"].as_array().unwrap().len(), 1);
        assert_eq!(changes["email.default_account"], Value::Null);
        // Removing another account leaves the default alone.
        let changes = applied(plan(&two(), &Change::Remove("b".into()), mint).unwrap());
        assert!(!changes.contains_key("email.default_account"));
    }

    #[test]
    fn moves_defaults_and_enables_send_the_list_or_the_default() {
        let changes =
            applied(plan(&two(), &Change::Move("b".into(), Direction::Up), mint).unwrap());
        let ids: Vec<_> = changes["email.accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(
            plan(&two(), &Change::Move("a".into(), Direction::Up), mint),
            Err(Refusal::Unchanged),
            "the first goes no higher"
        );
        assert_eq!(
            plan(&two(), &Change::Move("b".into(), Direction::Down), mint),
            Err(Refusal::Unchanged)
        );
        let changes = applied(plan(&two(), &Change::Default(Some("b".into())), mint).unwrap());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes["email.default_account"], "b");
        assert_eq!(
            plan(&two(), &Change::Default(Some("zz".into())), mint),
            Err(Refusal::Key("email-unknown-account"))
        );
        let changes = applied(plan(&two(), &Change::Enable("b".into(), false), mint).unwrap());
        assert_eq!(changes["email.accounts"][1]["enabled"], false);
        assert_eq!(
            plan(&two(), &Change::Enable("b".into(), true), mint),
            Err(Refusal::Unchanged)
        );
    }

    #[test]
    fn compose_paths_parse_and_refuse_bad_values() {
        assert!(matches!(
            email_change(&json!({"path": "email.mail.quote_on_reply", "value": false})),
            Ok(Change::QuoteOnReply(false))
        ));
        assert!(matches!(
            email_change(&json!({"path": "email.mail.reply_position", "value": "below"})),
            Ok(Change::ReplyPosition(ReplyPosition::Below))
        ));
        assert!(
            email_change(&json!({"path": "email.mail.reply_position", "value": "middle"})).is_err()
        );
        assert!(
            email_change(&json!({"path": "email.mail.quote_on_reply", "value": "yes"})).is_err()
        );
    }

    #[test]
    fn mail_options_send_their_own_path() {
        let email = two();
        let cases = [
            (Change::PageSize(250), "email.mail.page_size", json!(250)),
            (
                Change::MarkReadOnOpen(false),
                "email.mail.mark_read_on_open",
                json!(false),
            ),
            (
                Change::ConfirmDelete(false),
                "email.mail.confirm_delete",
                json!(false),
            ),
            (
                Change::QuoteOnReply(false),
                "email.mail.quote_on_reply",
                json!(false),
            ),
            (
                Change::ReplyPosition(ReplyPosition::Below),
                "email.mail.reply_position",
                json!("below"),
            ),
        ];
        for (change, path, value) in cases {
            let changes = applied(plan(&email, &change, mint).unwrap());
            assert_eq!(changes, BTreeMap::from([(path.to_owned(), value)]));
        }
        for size in [PAGE_SIZE_MIN - 1, PAGE_SIZE_MAX + 1] {
            assert_eq!(
                plan(&email, &Change::PageSize(size), mint),
                Err(Refusal::Key("email-page-size-refused"))
            );
        }
    }

    #[test]
    fn bus_arguments_parse_into_changes() {
        assert_eq!(
            email_change(&json!({"op":"move","id":"a","direction":"down"})),
            Ok(Change::Move("a".into(), Direction::Down))
        );
        assert_eq!(
            email_change(&json!({"op":"default","id":null})),
            Ok(Change::Default(None))
        );
        assert_eq!(
            email_change(&json!({"path":"email.mail.page_size","value":50.0})),
            Ok(Change::PageSize(50))
        );
        assert_eq!(
            email_change(&json!({"op":"update","id":"a","ca_path":null})),
            Ok(Change::Update(
                "a".into(),
                Patch {
                    ca_path: Some(None),
                    ..Default::default()
                }
            ))
        );
        for bad in [
            json!({}),
            json!({"op":"add","name":"x"}),
            json!({"op":"update","id":"a","password":"hunter2"}),
            json!({"op":"enable","id":"a"}),
            json!({"path":"email.accounts","value":[]}),
        ] {
            assert!(email_change(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls_and_text_are_redacted_for_observation() {
        for (typed, shown) in [
            ("https://mail.example.org", "https://mail.example.org"),
            (
                "https://mail.example.org:8443/jmap",
                "https://mail.example.org:8443/jmap",
            ),
            (
                "https://user:hunter2@mail.example.org/jmap",
                "https://[redacted]@mail.example.org/jmap",
            ),
            (
                "https://a@b:c@mail.example.org",
                "https://[redacted]@mail.example.org",
            ),
            (
                "https://mail.example.org/p?token=abc&page=2#top",
                "https://mail.example.org/p?token=[redacted]&page=2#top",
            ),
            (
                "https://mail.example.org?Password=x&API_KEY=y",
                "https://mail.example.org?Password=[redacted]&API_KEY=[redacted]",
            ),
            (
                "user:hunter2@mail.example.org",
                "[redacted]@mail.example.org",
            ),
            ("", ""),
        ] {
            assert_eq!(redact_url(typed), shown, "{typed}");
        }
        assert_eq!(
            redact_text("refused \"https://u:p@h.example/x\", try again"),
            "refused \"https://[redacted]@h.example/x\", try again"
        );
        assert_eq!(redact_text("no url here: a@b"), "no url here: a@b");
    }

    /// sol re-review #5: a URL with a password pasted into the form never
    /// reaches prefs.email, prefs.info, Debug or the status line, before or
    /// after Save refuses it; only the edit field holds it as typed.
    #[test]
    fn a_typed_userinfo_url_is_never_observed() {
        let leaked = |text: &str| text.contains("hunter2") || text.contains("s3cret");
        let observed = |e: &mut Engine| -> Vec<String> {
            e.command(90, "prefs.email", "{}");
            e.command(91, "prefs.info", "{}");
            let mut out: Vec<String> = e
                .take_effects()
                .iter()
                .filter_map(|x| match x {
                    Effect::Reply { body, .. } => Some(body.to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(out.len(), 2);
            out.push(format!("{:?}", e.email));
            out.push(format!("{:?}", e.email.form.as_ref().map(Form::patch)));
            out.push(e.status.clone());
            out
        };
        let mut e = read();
        e.new_email_account();
        e.edit_email_field(Field::Name, "Leak".into());
        e.edit_email_field(
            Field::Url,
            "https://user:hunter2@mail.example.org/jmap?password=s3cret".into(),
        );
        e.edit_email_field(Field::Email, "user@example.org".into());
        for text in observed(&mut e) {
            assert!(!leaked(&text), "{text}");
        }
        assert!(
            e.email.form.as_ref().unwrap().url.contains("hunter2"),
            "the edit field keeps what was typed"
        );
        e.save_email_account();
        assert!(settings_calls(&mut e).is_empty(), "refused before sending");
        assert!(e.failed);
        assert!(
            e.email
                .form
                .as_ref()
                .unwrap()
                .errors
                .contains_key(&Field::Url)
        );
        for text in observed(&mut e) {
            assert!(!leaked(&text), "{text}");
        }
        assert_eq!(
            e.email_view()["form"]["url"],
            "https://[redacted]@mail.example.org/jmap?password=[redacted]"
        );
        // The same URL sent over the Bus is refused without an echo.
        e.command(
            92,
            "prefs.email.set",
            r#"{"op":"add","name":"x","url":"https://user:hunter2@mail.example.org","email":"x@example.org"}"#,
        );
        for x in e.take_effects() {
            assert!(!leaked(&format!("{x:?}")), "{x:?}");
        }
        assert!(!leaked(&e.status));
    }

    #[test]
    fn a_secret_never_prints() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(..)");
        assert_eq!(
            format!("{:?}", Action::Password("a".into(), secret.clone())),
            "Password(\"a\", Secret(..))"
        );
        assert_eq!(secret.expose(), "hunter2");
    }

    // ---- the engine --------------------------------------------------------

    fn reply(rc: u8, body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc,
            body: body.to_string(),
            error: None,
        })
    }

    fn missing() -> Result<Reply, CallError> {
        Ok(Reply {
            rc: 10,
            body: String::new(),
            error: Some("Service 'email' not found".into()),
        })
    }

    fn snapshot(revision: &str, email: Value) -> Value {
        json!({"status":"current","summary":{"schema":1,
            "binding":{"instance":"example","profile":"default"},
            "incarnation":"inc-1","revision":revision,"design_revision":revision,
            "source_digest":"d","appearance":{"scheme":"studio","mode":"dark","contrast":"normal","source":null},
            "custom_source":false,"email":email},
            "publication_pending":false,"recovering":false})
    }

    fn label(key: &str, args: &[(&str, &str)]) -> String {
        let mut out = key.to_owned();
        for (k, v) in args {
            out.push_str(&format!(" {k}={v}"));
        }
        out
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

    fn mail_calls(effects: &[Effect]) -> Vec<(u64, &'static str, &'static str, Value, bool)> {
        effects
            .iter()
            .filter_map(|x| match x {
                Effect::Mail {
                    ticket,
                    service,
                    verb,
                    body,
                    secret,
                } => Some((*ticket, *service, *verb, body.clone(), secret.is_some())),
                _ => None,
            })
            .collect()
    }

    /// An engine whose settingsd read holds two accounts, Work the default,
    /// with the Email section shown.
    fn read() -> Engine {
        let mut e = Engine::new(label);
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
        e.settled(calls[0].0, reply(0, snapshot("7", json!(two()))));
        e.set_section(Section::Email);
        // Email is up and knows of no passwords yet.
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].2, "email.accounts");
        e.mailed(calls[0].0, reply(0, json!({"accounts":[]})));
        e.take_effects();
        e
    }

    fn settle_read(e: &mut Engine, revision: &str, email: Value) {
        let reads = settings_calls(e);
        assert_eq!(reads.len(), 1, "{reads:?}");
        assert_eq!(reads[0].1, "settings.get");
        e.settled(reads[0].0, reply(0, snapshot(revision, email)));
    }

    #[test]
    fn the_read_fills_the_section_and_the_view_has_no_secret() {
        let mut e = read();
        assert_eq!(e.email().map(|m| m.accounts.len()), Some(2));
        assert!(e.can_edit_email());
        e.select_email_account("b");
        e.edit_email_password(Secret::new("hunter2"));
        let view = e.email_view();
        assert_eq!(view["accounts"][0]["default"], true);
        assert_eq!(view["accounts"][1]["has_password"], Value::Null);
        assert_eq!(view["password_typed"], true);
        assert!(!view.to_string().contains("hunter2"));
        assert!(!e.info().to_string().contains("hunter2"));
    }

    #[test]
    fn removing_the_default_is_one_fenced_apply_after_a_confirm() {
        let mut e = read();
        // Email away: the removal goes straight to settingsd.
        e.email.service = Service::Absent;
        e.select_email_account("a");
        e.remove_email_account();
        assert_eq!(e.ui.dialog, Some(Dialog::RemoveAccount("a".into())));
        assert!(
            settings_calls(&mut e).is_empty(),
            "nothing before the confirm"
        );
        e.close_dialog();
        e.confirm_email();
        assert!(
            settings_calls(&mut e).is_empty(),
            "a closed dialog confirms nothing"
        );
        e.remove_email_account();
        e.confirm_email();
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        let (ticket, verb, body) = calls[0].clone();
        assert_eq!(verb, "settings.apply");
        assert_eq!(body["expected_revision"], "7");
        assert_eq!(body["changes"]["email.default_account"], Value::Null);
        assert_eq!(
            body["changes"]["email.accounts"].as_array().unwrap().len(),
            1
        );
        assert!(!e.can_edit_email(), "one apply at a time");
        e.settled(ticket, reply(0, json!({"status":"changed","receipt":{}})));
        let mut after = two();
        after.accounts.remove(0);
        after.default_account = None;
        settle_read(&mut e, "8", json!(after));
        assert!(e.email.form.is_none(), "the removed account's form closes");
        assert_eq!(e.ui.email_account, None);
        assert!(e.can_edit_email());
    }

    #[test]
    fn a_new_account_is_validated_then_added_and_selected() {
        let mut e = read();
        e.new_email_account();
        e.edit_email_field(Field::Name, "Gold Coast".into());
        e.edit_email_field(Field::Url, "http://mail.example.com".into());
        e.edit_email_field(Field::Email, "admin@example.com".into());
        e.save_email_account();
        assert!(settings_calls(&mut e).is_empty(), "a bad URL is not sent");
        let form = e.email.form.as_ref().unwrap();
        assert_eq!(
            form.errors,
            Refusals::from([(Field::Url, "email-url-refused")])
        );
        assert!(e.failed);
        e.edit_email_field(Field::Url, "https://mail.example.com".into());
        assert!(
            e.email.form.as_ref().unwrap().errors.is_empty(),
            "typing clears it"
        );
        e.save_email_account();
        let calls = settings_calls(&mut e);
        let list = calls[0].2["changes"]["email.accounts"].clone();
        let id = list[2]["id"].as_str().unwrap().to_owned();
        assert!(id.starts_with("acct-"));
        e.settled(
            calls[0].0,
            reply(0, json!({"status":"changed","receipt":{}})),
        );
        let mut after = two();
        after
            .accounts
            .push(serde_json::from_value(list[2].clone()).unwrap());
        settle_read(&mut e, "8", json!(after));
        assert_eq!(e.ui.email_account.as_deref(), Some(id.as_str()));
        assert_eq!(
            e.email.form.as_ref().unwrap().id.as_deref(),
            Some(id.as_str())
        );
    }

    #[test]
    fn a_refused_apply_keeps_the_form_and_settingsd_s_values() {
        let mut e = read();
        e.select_email_account("b");
        e.edit_email_field(Field::Name, "Family".into());
        e.save_email_account();
        let calls = settings_calls(&mut e);
        e.settled(
            calls[0].0,
            reply(
                10,
                json!({"status":"invalid","diagnostics":[{"path":"email.accounts[1].name","message":"Name must be a plain label"}]}),
            ),
        );
        assert!(e.failed);
        assert!(
            e.status.contains("Name must be a plain label"),
            "{}",
            e.status
        );
        settle_read(&mut e, "7", json!(two()));
        assert_eq!(
            e.email().unwrap().accounts[1].name,
            "Home",
            "settingsd's value"
        );
        assert_eq!(
            e.email.form.as_ref().unwrap().name,
            "Family",
            "the typing stays"
        );
        assert!(e.can_revert_email_account());
        e.revert_email_account();
        assert_eq!(e.email.form.as_ref().unwrap().name, "Home");
    }

    #[test]
    fn an_unedited_form_follows_another_writer() {
        let mut e = read();
        e.select_email_account("b");
        e.delivery(citizen::Delivery::Topic(crate::appearance::topic()));
        let mut after = two();
        after.accounts[1].name = "Family".into();
        settle_read(&mut e, "8", json!(after));
        assert_eq!(e.email.form.as_ref().unwrap().name, "Family");
    }

    #[test]
    fn reset_asks_first_and_resets_only_the_email_group() {
        let mut e = read();
        e.reset_email();
        assert_eq!(e.ui.dialog, Some(Dialog::ResetEmail));
        e.confirm_email();
        let calls = settings_calls(&mut e);
        assert_eq!(calls[0].1, "settings.reset");
        assert_eq!(calls[0].2["changes"], json!({}));
        assert_eq!(calls[0].2["reset"], json!(["email"]));
    }

    #[test]
    fn entering_the_section_asks_email_about_passwords() {
        let mut e = read();
        e.set_section(Section::Themes);
        e.take_effects();
        e.set_section(Section::Email);
        let effects = e.take_effects();
        let calls = mail_calls(&effects);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].1, calls[0].2), (EMAIL, "email.accounts"));
        e.mailed(
            calls[0].0,
            reply(0, json!({"accounts":[{"id":"a","has_password":true},{"id":"b","has_password":false}]})),
        );
        assert_eq!(e.email.service, Service::Running);
        assert_eq!(e.email_view()["accounts"][0]["has_password"], true);
        e.refresh();
        let calls = mail_calls(&e.take_effects());
        e.mailed(calls[0].0, missing());
        assert_eq!(e.email.service, Service::Absent);
        assert!(
            e.email.passwords.is_empty(),
            "not known while Email is away"
        );
    }

    /// The password goes to Email in the effect's secret, never its body;
    /// a set password leaves memory and the Bus command is answered.
    #[test]
    fn set_password_launches_email_when_absent_then_sends_once_it_registers() {
        let mut e = read();
        e.select_email_account("a");
        e.edit_email_password(Secret::new("hunter2"));
        assert!(e.can_set_email_password());
        e.set_email_password();
        assert!(e.email.password.is_empty(), "the field empties as it sends");
        let effects = e.take_effects();
        let calls = mail_calls(&effects);
        assert_eq!(calls[0].2, "email.account.password.set");
        assert_eq!(calls[0].3, json!({"id":"a"}), "no password in the body");
        assert!(calls[0].4, "the secret rides beside it");
        assert!(!format!("{effects:?}").contains("hunter2"));
        e.mailed(calls[0].0, missing());
        let calls = mail_calls(&e.take_effects());
        assert_eq!((calls[0].1, calls[0].2), (APPS, "apps.launch"));
        assert_eq!(calls[0].3, json!({"id":EMAIL_APP_ID}));
        e.mailed(calls[0].0, reply(0, json!({"launched":true})));
        let wait = e
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Wait { ticket, after_ms } => Some((ticket, after_ms)),
                _ => None,
            })
            .unwrap();
        assert_eq!(wait.1, LAUNCH_WAITS_MS[0]);
        assert!(e.busy());
        // Email registers before the wait is over: it is tried at once, and
        // the timer's late firing is ignored.
        e.delivery(citizen::Delivery::Changed);
        let effects = e.take_effects();
        let calls = mail_calls(&effects);
        let set = calls
            .iter()
            .find(|c| c.2 == "email.account.password.set")
            .expect("tried again");
        e.waited(wait.0);
        assert!(mail_calls(&e.take_effects()).is_empty());
        e.mailed(set.0, reply(0, json!({})));
        assert!(e.email.task.is_none());
        assert!(!e.failed);
        assert!(e.status.starts_with("email-password-set"), "{}", e.status);
    }

    #[test]
    fn a_launch_that_never_registers_gives_up_and_returns_the_password() {
        let mut e = read();
        e.select_email_account("a");
        e.edit_email_password(Secret::new("hunter2"));
        e.set_email_password();
        let calls = mail_calls(&e.take_effects());
        e.mailed(calls[0].0, missing());
        let calls = mail_calls(&e.take_effects());
        e.mailed(calls[0].0, reply(0, json!({})));
        let mut tries = 0;
        loop {
            let effects = e.take_effects();
            let Some(ticket) = effects.iter().find_map(|x| match x {
                Effect::Wait { ticket, .. } => Some(*ticket),
                _ => None,
            }) else {
                break;
            };
            e.waited(ticket);
            let calls = mail_calls(&e.take_effects());
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].2, "email.account.password.set", "no second launch");
            tries += 1;
            e.mailed(calls[0].0, missing());
        }
        assert_eq!(tries, LAUNCH_WAITS_MS.len(), "bounded");
        assert!(e.email.task.is_none() && e.failed);
        assert!(e.status.starts_with("email-did-not-start"));
        assert_eq!(e.email.password.expose(), "hunter2", "back in the field");
    }

    #[test]
    fn an_email_without_the_verb_is_reported_not_retried() {
        let mut e = read();
        e.select_email_account("b");
        e.connect_email_account();
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].2, "email.account.connect");
        e.mailed(
            calls[0].0,
            reply(
                10,
                json!({"error_code":"UNKNOWN_VERB","message":"unknown Email verb"}),
            ),
        );
        assert!(e.failed && e.status == "email-unsupported");
        assert!(mail_calls(&e.take_effects()).is_empty());
        assert!(e.can_connect_email_account());
    }

    /// Forget password asks first (Cancel the default), then sends the one
    /// empty password, launching Email when it is absent like Set password.
    #[test]
    fn forget_asks_first_then_sends_an_empty_password_through_the_launch_path() {
        let mut e = read();
        e.select_email_account("a");
        e.forget_email_password();
        assert_eq!(e.ui.dialog, Some(Dialog::ForgetPassword("a".into())));
        assert!(
            mail_calls(&e.take_effects()).is_empty(),
            "nothing before the confirm"
        );
        e.close_dialog();
        e.confirm_email();
        assert!(mail_calls(&e.take_effects()).is_empty());
        e.forget_email_password();
        e.confirm_email();
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].2, "email.account.password.set");
        assert_eq!(calls[0].3, json!({"id":"a","password":""}));
        assert!(!calls[0].4, "no secret rides with a forget");
        e.mailed(calls[0].0, missing());
        let calls = mail_calls(&e.take_effects());
        assert_eq!((calls[0].1, calls[0].2), (APPS, "apps.launch"));
        e.mailed(
            calls[0].0,
            reply(0, json!({"argv":[],"id":EMAIL_APP_ID,"pid":7})),
        );
        let wait = e
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Wait { ticket, .. } => Some(ticket),
                _ => None,
            })
            .unwrap();
        e.waited(wait);
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].3, json!({"id":"a","password":""}));
        e.mailed(calls[0].0, reply(0, json!({"id":"a","has_password":false})));
        assert!(!e.failed);
        assert!(
            e.status.starts_with("email-password-forgotten"),
            "{}",
            e.status
        );
    }

    /// An empty password goes out only as a forget: typed Set password and
    /// `prefs.email.password` with an empty password refuse it.
    #[test]
    fn an_empty_password_is_only_ever_a_forget() {
        let mut e = read();
        e.select_email_account("a");
        e.edit_email_password(Secret::new(""));
        assert!(!e.can_set_email_password());
        e.set_email_password();
        assert!(mail_calls(&e.take_effects()).is_empty());
        let refused = |e: &mut Engine, id: u64| {
            e.take_effects().iter().any(
                |x| matches!(x, Effect::Reply { id: i, rc: 10, body } if *i == id && body["error_code"] == "ARGUMENT"),
            )
        };
        e.command(1, "prefs.email.password", r#"{"id":"a","password":""}"#);
        assert!(refused(&mut e, 1));
        e.command(
            2,
            "prefs.email.password",
            r#"{"id":"a","forget":true,"password":"x"}"#,
        );
        assert!(refused(&mut e, 2));
        e.command(3, "prefs.email.password", r#"{"id":"a","forget":"yes"}"#);
        assert!(refused(&mut e, 3));
        // A forget takes any valid id, a removed account's too.
        e.command(
            4,
            "prefs.email.password",
            r#"{"id":"acct-gone","forget":true}"#,
        );
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].3, json!({"id":"acct-gone","password":""}));
        e.mailed(
            calls[0].0,
            reply(0, json!({"id":"acct-gone","has_password":false})),
        );
        assert!(
            e.take_effects()
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 4, rc: 0, .. }))
        );
    }

    /// An Email too old to remove accounts: the removal goes straight to
    /// settingsd, and once settingsd shows it gone, a running Email is asked
    /// to forget the password, never launched for it; a failure is a note,
    /// and Email not knowing the account is nothing to report.
    #[test]
    fn a_removed_account_s_password_is_forgotten_by_a_running_email() {
        let removed = |e: &mut Engine| {
            e.select_email_account("b");
            e.remove_email_account();
            e.confirm_email();
            if e.email.service != Service::Absent {
                let calls = mail_calls(&e.take_effects());
                assert_eq!(calls[0].2, "email.account.remove");
                e.mailed(
                    calls[0].0,
                    reply(
                        10,
                        json!({"error_code":"UNKNOWN_VERB","message":"unknown verb"}),
                    ),
                );
            }
            let calls = settings_calls(e);
            e.settled(calls[0].0, reply(0, json!({"status":"changed"})));
            let mut after = two();
            after.accounts.remove(1);
            settle_read(e, "8", json!(after));
        };
        let mut e = read();
        removed(&mut e);
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].3, json!({"id":"b","password":""}));
        e.mailed(calls[0].0, missing());
        assert!(
            mail_calls(&e.take_effects()).is_empty(),
            "Email is not launched for it"
        );
        assert!(e.failed);
        assert!(
            e.status
                .starts_with("email-forget-after-remove-failed account=Home"),
            "{}",
            e.status
        );
        // Email no longer knowing the account held nothing to forget.
        let mut e = read();
        removed(&mut e);
        let calls = mail_calls(&e.take_effects());
        e.mailed(
            calls[0].0,
            reply(
                10,
                json!({"error_code":"UNKNOWN_ACCOUNT","message":"no such account"}),
            ),
        );
        assert!(!e.failed, "{}", e.status);
        // Email known to be away: a note, no call.
        let mut e = read();
        e.email.service = Service::Absent;
        removed(&mut e);
        assert!(mail_calls(&e.take_effects()).is_empty());
        assert!(e.failed && e.email.outcome.as_ref().is_some_and(|(_, f)| *f));
    }

    #[test]
    fn a_refused_removal_forgets_nothing() {
        let mut e = read();
        e.email.service = Service::Absent;
        e.select_email_account("b");
        e.remove_email_account();
        e.confirm_email();
        let calls = settings_calls(&mut e);
        e.settled(
            calls[0].0,
            reply(10, json!({"status":"invalid","message":"no"})),
        );
        settle_read(&mut e, "7", json!(two()));
        assert!(mail_calls(&e.take_effects()).is_empty());
        assert!(e.email.outcome.is_none(), "no forget, no note");
    }

    /// Email may be running: Remove is Email's guarded email.account.remove
    /// (it updates settingsd and forgets the password itself), never a
    /// settingsd apply of Prefs' own, and no follow-up forget.
    #[test]
    fn remove_goes_through_email_when_it_runs() {
        let mut e = read();
        e.select_email_account("b");
        e.remove_email_account();
        e.confirm_email();
        let effects = e.take_effects();
        assert!(
            !effects.iter().any(|x| matches!(x, Effect::Settings { .. })),
            "{effects:?}"
        );
        let calls = mail_calls(&effects);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].1, calls[0].2), (EMAIL, "email.account.remove"));
        assert_eq!(calls[0].3, json!({"id":"b"}));
        assert!(!e.can_remove_email_account(), "one at a time");
        e.mailed(calls[0].0, reply(0, json!({"id":"b"})));
        assert!(!e.failed);
        assert!(
            e.status.starts_with("email-removed account=Home"),
            "{}",
            e.status
        );
        let mut after = two();
        after.accounts.remove(1);
        settle_read(&mut e, "8", json!(after));
        assert!(e.email.form.is_none());
        let calls = mail_calls(&e.take_effects());
        assert!(
            !calls.iter().any(|c| c.2 == "email.account.password.set"),
            "Email forgot it itself: {calls:?}"
        );
    }

    /// sol #15: Email refuses to remove an account mid-migration. Its answer
    /// is shown, and given to a Bus caller, as Email gave it; nothing is
    /// removed behind its back.
    #[test]
    fn a_migration_pending_refusal_is_shown_verbatim_and_nothing_removed() {
        let pending = json!({"error_code":"MIGRATION_PENDING","message":"The account is still being migrated"});
        let mut e = read();
        e.select_email_account("b");
        e.remove_email_account();
        e.confirm_email();
        let calls = mail_calls(&e.take_effects());
        e.mailed(calls[0].0, reply(10, pending.clone()));
        assert!(e.failed);
        assert!(
            e.status
                .contains("MIGRATION_PENDING: The account is still being migrated"),
            "{}",
            e.status
        );
        assert!(settings_calls(&mut e).is_empty(), "no settingsd removal");
        assert_eq!(e.email().unwrap().accounts.len(), 2);
        // The same over the Bus.
        e.command(1, "prefs.email.set", r#"{"op":"remove","id":"b"}"#);
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].2, "email.account.remove");
        e.mailed(calls[0].0, reply(10, pending));
        let effects = e.take_effects();
        assert!(effects.iter().any(|x| matches!(x,
            Effect::Reply { id: 1, rc: 10, body } if body["error_code"] == "MIGRATION_PENDING")));
        assert!(!effects.iter().any(|x| matches!(x, Effect::Settings { .. })));
    }

    /// Email away, or gone mid-remove: the settingsd removal instead, the
    /// Bus caller answered once it is sent.
    #[test]
    fn without_email_the_removal_goes_to_settingsd() {
        let mut e = read();
        e.email.service = Service::Absent;
        e.command(1, "prefs.email.set", r#"{"op":"remove","id":"b"}"#);
        let effects = e.take_effects();
        assert!(
            mail_calls(&effects).is_empty(),
            "Email is not asked or launched"
        );
        assert!(effects.iter().any(|x| matches!(
            x,
            Effect::Settings {
                verb: "settings.apply",
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 1, rc: 0, .. }))
        );
        // Email registered, then gone by the time it is asked.
        let mut e = read();
        e.command(2, "prefs.email.set", r#"{"op":"remove","id":"b"}"#);
        let calls = mail_calls(&e.take_effects());
        assert_eq!(calls[0].2, "email.account.remove");
        e.mailed(calls[0].0, missing());
        let effects = e.take_effects();
        assert!(effects.iter().any(|x| matches!(
            x,
            Effect::Settings {
                verb: "settings.apply",
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 2, rc: 0, .. }))
        );
        assert!(
            !effects
                .iter()
                .any(|x| matches!(x, Effect::Mail { service: APPS, .. })),
            "never launched for it"
        );
        // An unknown account is refused at once.
        let mut e = read();
        e.command(3, "prefs.email.set", r#"{"op":"remove","id":"zz"}"#);
        assert!(e.take_effects().iter().any(|x| matches!(x,
            Effect::Reply { id: 3, rc: 10, body } if body["error_code"] == "ARGUMENT")));
    }

    #[test]
    fn the_bus_verbs_take_the_same_paths() {
        let mut e = read();
        e.command(1, "prefs.email.set", r#"{"op":"default","id":"b"}"#);
        let effects = e.take_effects();
        let apply = effects
            .iter()
            .find_map(|x| match x {
                Effect::Settings { ticket, body, .. } => Some((*ticket, body.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(apply.1["changes"], json!({"email.default_account":"b"}));
        assert!(
            effects
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 1, rc: 0, .. }))
        );
        e.command(
            2,
            "prefs.email.set",
            r#"{"op":"enable","id":"b","enabled":false}"#,
        );
        assert!(e.take_effects().iter().any(
            |x| matches!(x, Effect::Reply { id: 2, rc: 10, body } if body["error_code"] == "BUSY")
        ));
        e.settled(apply.0, reply(0, json!({"status":"changed"})));
        settle_read(&mut e, "8", json!(two()));
        e.command(
            3,
            "prefs.email.set",
            r#"{"op":"add","name":"x","url":"ftp://x","email":"x@y"}"#,
        );
        assert!(e.take_effects().iter().any(
            |x| matches!(x, Effect::Reply { id: 3, rc: 10, body } if body["message"].as_str().unwrap().starts_with("url:"))
        ));
        e.command(4, "prefs.email", "{}");
        assert!(e.take_effects().iter().any(
            |x| matches!(x, Effect::Reply { id: 4, rc: 0, body } if body["accounts"].as_array().unwrap().len() == 2)
        ));
        // A password over the Bus is answered when Email has answered.
        e.command(
            5,
            "prefs.email.password",
            r#"{"id":"a","password":"hunter2"}"#,
        );
        let effects = e.take_effects();
        assert!(
            !effects
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 5, .. }))
        );
        let calls = mail_calls(&effects);
        e.mailed(calls[0].0, reply(0, json!({})));
        assert!(
            e.take_effects()
                .iter()
                .any(|x| matches!(x, Effect::Reply { id: 5, rc: 0, .. }))
        );
        e.command(6, "prefs.email.password", r#"{"id":"zz","password":"x"}"#);
        assert!(e.take_effects().iter().any(
            |x| matches!(x, Effect::Reply { id: 6, rc: 10, body } if body["error_code"] == "ARGUMENT")
        ));
    }
}
