// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The renderer-independent model: the rows and notes the
//! `releases` service answers (docs/docs/spec/releases/), and Preferences' own commands.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const APP_ID: &str = "dev.mixos.prefs";
/// The Bus service behind Software.
pub const RELEASES: &str = "releases";

/// What a row's status says, as the panel draws and enables it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowState {
    Current,
    Update,
    NotInstalled,
    Unchecked,
    Error,
}

/// One app as `releases.list` / `releases.check` answer it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppRow {
    pub app: String,
    pub repo: String,
    /// Optional catalogue section; old releases services omit this field.
    #[serde(default)]
    pub category: Option<String>,
    /// Optional catalogue display label for the category.
    #[serde(default)]
    pub category_label: Option<String>,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub published: Option<String>,
    pub status: String,
}

impl AppRow {
    pub fn state(&self) -> RowState {
        match self.status.as_str() {
            "current" => RowState::Current,
            "update" => RowState::Update,
            "not installed" => RowState::NotInstalled,
            "unchecked" if self.installed.is_some() => RowState::Current,
            "unchecked" => RowState::Unchecked,
            _ => RowState::Error,
        }
    }

    /// Something to install or update to.
    pub fn installable(&self) -> bool {
        matches!(self.state(), RowState::NotInstalled | RowState::Update)
            || (self.installed.is_none() && self.state() == RowState::Unchecked)
    }

    pub fn installed(&self) -> bool {
        self.installed.is_some()
    }
}

/// The rows of a `releases.list` / `releases.check` reply; a row that does
/// not parse is an error, not silently dropped.
pub fn rows(value: &Value) -> Result<Vec<AppRow>, String> {
    let list = value.as_array().ok_or("expected a list of rows")?;
    list.iter()
        .map(|row| serde_json::from_value(row.clone()).map_err(|e| format!("row: {e}")))
        .collect()
}

/// The latest release's notes, as `releases.notes` answers them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    pub app: String,
    pub tag: String,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub notes: String,
}

/// Every command the engine answers itself. The window-level commands
/// (`prefs.ui.*`, `prefs.window*`) belong to the toolkit drive layer.
pub const COMMANDS: [&str; 19] = [
    "HELP",
    "app.describe",
    "prefs.ping",
    "prefs.info",
    "prefs.show",
    "prefs.actions",
    "prefs.execute",
    "prefs.quit",
    "prefs.search",
    "prefs.section",
    "prefs.select",
    "prefs.apps",
    "prefs.appearance",
    "prefs.appearance.set",
    "prefs.dialog",
    "prefs.theme",
    "prefs.email",
    "prefs.email.set",
    "prefs.email.password",
];

/// The engine's Bus surface (`HELP` / `app.describe` add the drive commands).
pub fn describe() -> Value {
    json!({"schema":"prefs.v1","app_id":APP_ID,"sections":crate::SECTIONS,"categories":crate::Category::ALL,"commands":[
        {"name":"HELP","description":"List every command","read_only":true},
        {"name":"app.describe","description":"Describe the app and every command with its arguments","read_only":true},
        {"name":"prefs.ping","description":"Probe Preferences","read_only":true},
        {"name":"prefs.info","version":"0.5.0","description":"The whole state: section, category, rows, selection, notes, operation in progress, settings status, release_status, release_failed and UI","read_only":true},
        {"name":"prefs.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"prefs.actions","description":"List the app's commands with labels, shortcuts and enablement","read_only":true},
        {"name":"prefs.execute","args":{"id":"string"},"description":"Run a command from prefs.actions, with the same enablement as its menu item, button or shortcut","read_only":false},
        {"name":"prefs.quit","description":"Close once the operation in progress finishes","read_only":false},
        {"name":"prefs.search","args":{"query":"string"},"description":"Find sections by title, category, alias or settings key without changing the window; returns matches with id and category in sidebar order, except system ranks Software first","read_only":true},
        {"name":"prefs.section","args":{"name":crate::SECTIONS.iter().map(|s| s.id).collect::<Vec<_>>(),"group":crate::SoftwareGroup::ALL.map(crate::SoftwareGroup::name)},"description":"Select a section; optional group selects a group within Software","read_only":false},
        {"name":"prefs.select","args":{"app":"an app name from prefs.apps"},"description":"Open an app's Software group, select it and load its release notes","read_only":false},
        {"name":"prefs.apps","version":"0.3.0","description":"Software's rows, group, selection, notes, release_status and release_failed","read_only":true},
        {"name":"prefs.appearance","version":"0.3.0","description":"The Appearance panel: the session look settingsd holds (current), the unapplied edits (draft, and the axes edited), axes another writer changed under them (conflicts: nothing applies until appearance.keep or appearance.revert), what the window shows (including effective decorations and decorations_reason), the revision read, and whether an apply is in flight or uncertain","read_only":true},
        {"name":"prefs.appearance.set","version":"0.3.0","args":{"scheme":"optional scheme name (ocean, crimson, stone, forest, sunset, mono, pro, studio, classic, breeze, adwaita)","style":"optional style (plain, pro, studio, classic, breeze, adwaita) or own for the scheme's own","mode":"optional light or dark","contrast":"optional normal or high","decorations":"optional auto, csd or ssd","caption_side":"optional right or left","font_family":"optional installed family name or null for Inter","mono_family":"optional monospace family name or null for JetBrains Mono","text_scale":"optional number from 0.5 to 3.0","density":"optional number from 0.5 to 2.0","reduced_motion":"optional boolean"},"description":"Change the session appearance: a present key sets that axis, an absent one leaves it; every axis is checked first, then the change applies at once (fenced), or as soon as an apply in flight settles; prefs.appearance reports a held refusal or a conflict","read_only":false},
        {"name":"prefs.dialog","args":{"open":"\"about\", \"shortcuts\" or null"},"description":"Open a dialog, or close the open one (a remove or reset confirmation is closed, never confirmed, here)","read_only":false},
        {"name":"prefs.theme","args":{"scheme":"optional scheme name (ocean, crimson, stone, forest, sunset, mono, pro, studio, classic, breeze, adwaita) or null to follow the session","style":"optional style (plain, pro, studio, classic, breeze, adwaita), own for the scheme's own, or null to follow the session","mode":"optional light, dark or null to follow the session","decorations":"optional auto, csd, ssd or null to follow the session","caption_side":"optional right, left or null to follow the session"},"description":"Choose the window's theme: a present key sets that axis, an absent one leaves it; answers the choice and the effective scheme, mode, decorations and decorations_reason","read_only":false},
        {"name":"prefs.email","description":"Apps → Email: the email app's accounts (in tree order, each with default and has_password: true, false, or null when Email is not running), default_account and mail options as settingsd last held them, Email's state on the Bus, the selected account and its form, and any password or connect in progress; never a password","read_only":true},
        {"name":"prefs.email.set","args":{"op":"add, update, remove, move, default or enable","id":"the account (not for add; null for default clears it)","name":"add/update: the label","url":"add/update: https:// JMAP base, no trailing slash","email":"add/update: the login address","ca_path":"add/update: absolute PEM path, or null for the system roots","signature":"add/update: plain text","enabled":"add/update/enable: true or false","direction":"move: up or down","path":"instead of op: email.mail.page_size, email.mail.mark_read_on_open, email.mail.confirm_delete, email.mail.quote_on_reply or email.mail.reply_position (\"above\" or \"below\")","value":"with path: the new value"},"description":"Change the email settings in settingsd at once, as the window would: one fenced settings.apply carrying the whole account list, with default_account in the same batch when the change touches it (removing the default clears it); remove goes through Email (email.account.remove, answered with Email's reply, MIGRATION_PENDING included) when it may be running, else straight to settingsd; every field is checked first and a refusal names it; add answers the new account's id","read_only":false},
        {"name":"prefs.email.password","args":{"id":"an account id","password":"the password (not empty)","forget":"optional true, instead of password: Email deletes the saved password (any account id, a removed one too)"},"description":"Hand an account's password to the email app (email.account.password.set), or with forget:true have Email delete it (the only empty password sent), launching Email first when it is not on the Bus; answered once Email has. Preferences neither stores nor shows it, and settingsd never sees it","read_only":false}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_command_is_described_once() {
        let described: Vec<_> = describe()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, COMMANDS);
    }

    #[test]
    fn rows_parse_the_releases_shape_and_refuse_garbage() {
        let rows = rows(&json!([
            {"app":"demo","repo":"o/demo","installed":"1.0","latest":"2.0","published":"2026-10-10","checked":"x","status":"update"},
            {"app":"new","repo":"o/new","installed":null,"latest":"1.0","published":null,"checked":null,"status":"not installed"},
            {"app":"odd","repo":"o/odd","installed":null,"latest":null,"published":null,"checked":null,"status":"error: GitHub answered 403"}
        ]))
        .unwrap();
        assert_eq!(rows[0].state(), RowState::Update);
        assert!(rows[0].installable() && rows[0].installed());
        assert_eq!(rows[1].state(), RowState::NotInstalled);
        assert!(rows[1].installable() && !rows[1].installed());
        assert_eq!(rows[2].state(), RowState::Error);
        assert!(super::rows(&json!({"app":"x"})).is_err());
        assert!(super::rows(&json!([{"app":"x"}])).is_err());
    }

    #[test]
    fn an_installed_unchecked_app_is_current_until_checked() {
        let row = AppRow {
            app: "a".into(),
            repo: "o/a".into(),
            category: None,
            category_label: None,
            installed: Some("1.0".into()),
            latest: None,
            published: None,
            status: "unchecked".into(),
        };
        assert_eq!(row.state(), RowState::Current);
        assert!(!row.installable());
    }
}
