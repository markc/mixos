// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The settings writer: one account change becomes one `settings.apply`
//! batch on the `email.*` group, computed against the settings the app last
//! read and validated by the settings crate's own rules before anything is
//! sent. A batch carries the whole new `email.accounts` array (the group
//! replaces it whole) and, when it changes, `email.default_account` in the
//! same batch, so settingsd never holds half a change: removing the default
//! account resets the default with it.
//!
//! This module is pure. The state ([`crate::state`]) holds each write in
//! flight and its Bus reply; the shell fences it on the look it read
//! (`settings::follow::Follower::apply`), so a change made meanwhile
//! elsewhere is refused as a conflict, and the state then retries on the
//! fresh read.

use serde_json::{Map, Value, json};
use settings::email::{AccountEntry, Email};

/// One change to the accounts.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Create (an id not yet held) or replace an account.
    Upsert(AccountEntry),
    /// Change only the fields given on an account. A retry after a
    /// conflict applies them to the fresh read, so a field another writer
    /// changed meanwhile and this change does not name keeps their value.
    Update(String, AccountPatch),
    /// Remove an account; the default resets with it.
    Remove(String),
    /// Make an account the compose default (`None`: the first enabled one).
    Default(Option<String>),
    /// The schema-1 account: added unless an account already has its URL
    /// and address (then that one is it), and made the default when there
    /// is none.
    Migrate(AccountEntry),
}

/// The fields of an account an update names; `None` leaves a field as the
/// fresh read has it. `ca_path: Some(None)` clears the path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountPatch {
    pub name: Option<String>,
    pub url: Option<String>,
    pub email: Option<String>,
    pub ca_path: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub signature: Option<String>,
}

impl AccountPatch {
    /// `entry` with the named fields set.
    pub fn apply(&self, entry: &mut AccountEntry) {
        if let Some(name) = &self.name {
            entry.name = name.clone();
        }
        if let Some(url) = &self.url {
            entry.url = url.clone();
        }
        if let Some(email) = &self.email {
            entry.email = email.clone();
        }
        if let Some(ca_path) = &self.ca_path {
            entry.ca_path = ca_path.clone();
        }
        if let Some(enabled) = self.enabled {
            entry.enabled = enabled;
        }
        if let Some(signature) = &self.signature {
            entry.signature = signature.clone();
        }
    }
}

impl Change {
    /// The account this change is about.
    pub fn account(&self) -> Option<&str> {
        match self {
            Change::Upsert(entry) | Change::Migrate(entry) => Some(&entry.id),
            Change::Remove(id) | Change::Update(id, _) => Some(id),
            Change::Default(id) => id.as_deref(),
        }
    }
}

/// Why a change cannot be written.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// No account has this id.
    UnknownAccount(String),
    /// The result would not validate: settingsd would refuse it.
    Invalid { path: String, message: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownAccount(id) => write!(f, "no account {id:?}"),
            Refusal::Invalid { path, message } => write!(f, "{path}: {message}"),
        }
    }
}

/// The settings after `change`, and the id the change is about (a
/// migration that found its account already there names that account).
pub fn apply(email: &Email, change: &Change) -> Result<(Email, Option<String>), Refusal> {
    let mut next = email.clone();
    let mut about = change.account().map(str::to_owned);
    match change {
        Change::Upsert(entry) => match next.accounts.iter_mut().find(|a| a.id == entry.id) {
            Some(existing) => *existing = entry.clone(),
            None => next.accounts.push(entry.clone()),
        },
        Change::Update(id, patch) => match next.accounts.iter_mut().find(|a| a.id == *id) {
            Some(existing) => patch.apply(existing),
            None => return Err(Refusal::UnknownAccount(id.clone())),
        },
        Change::Remove(id) => {
            let before = next.accounts.len();
            next.accounts.retain(|a| a.id != *id);
            if next.accounts.len() == before {
                return Err(Refusal::UnknownAccount(id.clone()));
            }
            if next.default_account.as_deref() == Some(id) {
                next.default_account = None;
            }
        }
        Change::Default(id) => {
            if let Some(id) = id
                && next.account(id).is_none()
            {
                return Err(Refusal::UnknownAccount(id.clone()));
            }
            next.default_account = id.clone();
        }
        Change::Migrate(entry) => {
            let found = next
                .accounts
                .iter()
                .find(|a| a.url == entry.url && a.email.eq_ignore_ascii_case(&entry.email))
                .map(|a| a.id.clone());
            let id = match found {
                Some(id) => id,
                None => {
                    next.accounts.push(entry.clone());
                    entry.id.clone()
                }
            };
            if next.default_account.is_none() {
                next.default_account = Some(id.clone());
            }
            about = Some(id);
        }
    }
    settings::email::validate(&next).map_err(|d| Refusal::Invalid {
        path: d.path.clone(),
        message: d.message.clone(),
    })?;
    Ok((next, about))
}

/// The `settings.apply` changes that turn `old` into `new`: the whole
/// accounts array and the default when they differ, and each mail option
/// that differs. Empty when nothing changes.
pub fn changes(old: &Email, new: &Email) -> Value {
    let mut out = Map::new();
    if old.accounts != new.accounts {
        out.insert("email.accounts".into(), json!(new.accounts));
    }
    if old.default_account != new.default_account {
        out.insert("email.default_account".into(), json!(new.default_account));
    }
    if old.mail.page_size != new.mail.page_size {
        out.insert("email.mail.page_size".into(), json!(new.mail.page_size));
    }
    if old.mail.mark_read_on_open != new.mail.mark_read_on_open {
        out.insert(
            "email.mail.mark_read_on_open".into(),
            json!(new.mail.mark_read_on_open),
        );
    }
    if old.mail.confirm_delete != new.mail.confirm_delete {
        out.insert(
            "email.mail.confirm_delete".into(),
            json!(new.mail.confirm_delete),
        );
    }
    Value::Object(out)
}

/// A schema-1 account's id: the same account always gets the same one, so
/// a migration interrupted after its secret was written and run again
/// writes the same secret, never a second.
pub fn legacy_id(url: &str, email: &str) -> String {
    // FNV-1a over the identity; the settings crate turns it into an id.
    let mut seed: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url
        .bytes()
        .chain([0])
        .chain(email.to_ascii_lowercase().bytes())
    {
        seed ^= u64::from(byte);
        seed = seed.wrapping_mul(0x0000_0100_0000_01b3);
    }
    settings::email::mint_account_id_from(&[], seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> AccountEntry {
        AccountEntry {
            id: id.into(),
            name: id.to_uppercase(),
            url: format!("https://{id}.example.invalid"),
            email: format!("me@{id}.example.invalid"),
            ca_path: None,
            enabled: true,
            signature: String::new(),
        }
    }

    fn two() -> Email {
        Email {
            accounts: vec![entry("a"), entry("b")],
            default_account: Some("a".into()),
            ..Email::default()
        }
    }

    #[test]
    fn an_upsert_appends_or_replaces_in_place() {
        let (next, id) = apply(&two(), &Change::Upsert(entry("c"))).unwrap();
        assert_eq!(id.as_deref(), Some("c"));
        assert_eq!(next.accounts.len(), 3);
        let mut renamed = entry("a");
        renamed.name = "Work".into();
        let (next, _) = apply(&two(), &Change::Upsert(renamed)).unwrap();
        assert_eq!(next.accounts[0].name, "Work");
        assert_eq!(next.accounts.len(), 2);
        let changed = changes(&two(), &next);
        assert_eq!(changed.as_object().unwrap().len(), 1, "{changed}");
        assert_eq!(changed["email.accounts"][0]["name"], "Work");
    }

    /// Review finding 11: an update retried on a fresh read keeps a field
    /// another writer changed and this one did not name.
    #[test]
    fn an_update_reapplies_only_its_fields_to_the_fresh_read() {
        let rename = Change::Update(
            "a".into(),
            AccountPatch {
                name: Some("Work".into()),
                ..AccountPatch::default()
            },
        );
        let mut fresh = two();
        fresh.accounts[0].signature = "set elsewhere".into();
        let (next, _) = apply(&fresh, &rename).unwrap();
        assert_eq!(next.accounts[0].name, "Work");
        assert_eq!(next.accounts[0].signature, "set elsewhere");
        assert_eq!(
            apply(&Email::default(), &rename),
            Err(Refusal::UnknownAccount("a".into()))
        );
    }

    #[test]
    fn removing_the_default_resets_it_in_the_same_batch() {
        let (next, _) = apply(&two(), &Change::Remove("a".into())).unwrap();
        assert_eq!(next.default_account, None);
        let changed = changes(&two(), &next);
        assert_eq!(changed["email.default_account"], Value::Null);
        assert_eq!(changed["email.accounts"].as_array().unwrap().len(), 1);
        assert!(
            changed
                .as_object()
                .unwrap()
                .contains_key("email.default_account")
        );
    }

    #[test]
    fn unknown_accounts_and_invalid_results_are_refused_before_sending() {
        assert_eq!(
            apply(&two(), &Change::Remove("z".into())),
            Err(Refusal::UnknownAccount("z".into()))
        );
        assert_eq!(
            apply(&two(), &Change::Default(Some("z".into()))),
            Err(Refusal::UnknownAccount("z".into()))
        );
        let mut bad = entry("c");
        bad.url = "http://plain.example.invalid".into();
        assert!(matches!(
            apply(&two(), &Change::Upsert(bad)),
            Err(Refusal::Invalid { path, .. }) if path == "email.accounts[2].url"
        ));
    }

    #[test]
    fn a_migration_adds_once_and_adopts_a_matching_account() {
        let (next, id) = apply(&Email::default(), &Change::Migrate(entry("m"))).unwrap();
        assert_eq!(id.as_deref(), Some("m"));
        assert_eq!(next.default_account.as_deref(), Some("m"));
        // The same account already there under another id: that one is it.
        let mut same = entry("m");
        same.id = "other".into();
        let (again, id) = apply(&next, &Change::Migrate(same)).unwrap();
        assert_eq!(id.as_deref(), Some("m"));
        assert_eq!(again, next, "nothing to write");
        assert_eq!(changes(&next, &again), json!({}));
    }

    #[test]
    fn a_legacy_id_is_stable_and_valid() {
        let a = legacy_id("https://mail.example.invalid", "Admin@example.invalid");
        assert_eq!(
            a,
            legacy_id("https://mail.example.invalid", "admin@example.invalid")
        );
        assert_ne!(
            a,
            legacy_id("https://other.example.invalid", "admin@example.invalid")
        );
        assert!(
            settings::email::valid_id(&a) && a.starts_with("acct-"),
            "{a}"
        );
    }
}
