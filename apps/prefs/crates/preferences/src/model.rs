// SPDX-License-Identifier: MIT OR Apache-2.0
//! The renderer-independent model: Prefs' panels, the rows and notes the
//! `releases` service answers (docs/spec/releases/), and Prefs' own verbs.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const APP_ID: &str = "dev.mixos.prefs";
/// The Bus service behind the Applications panel.
pub const RELEASES: &str = "releases";

/// The editors Prefs holds, in sidebar order. Appearance and the rest join
/// here as they are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Panel {
    Applications,
}

impl Panel {
    pub const ALL: [Panel; 1] = [Panel::Applications];

    pub fn name(self) -> &'static str {
        match self {
            Panel::Applications => "applications",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

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

/// Every verb the engine answers itself. The window-level verbs
/// (`prefs.ui.*`, `prefs.window*`) belong to the toolkit drive layer.
pub const VERBS: [&str; 13] = [
    "HELP",
    "app.describe",
    "prefs.ping",
    "prefs.info",
    "prefs.show",
    "prefs.commands",
    "prefs.execute",
    "prefs.quit",
    "prefs.panel",
    "prefs.select",
    "prefs.apps",
    "prefs.dialog",
    "prefs.theme",
];

/// The engine's Bus surface (`HELP` / `app.describe` add the drive verbs).
pub fn describe() -> Value {
    json!({"schema":"prefs.v1","app_id":APP_ID,"verbs":[
        {"name":"HELP","description":"List every verb","read_only":true},
        {"name":"app.describe","description":"Describe the app and every verb with its arguments","read_only":true},
        {"name":"prefs.ping","description":"Probe Prefs","read_only":true},
        {"name":"prefs.info","description":"The whole state: panel, rows, selection, notes, operation in progress, status and UI","read_only":true},
        {"name":"prefs.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"prefs.commands","description":"List the app's commands with labels, shortcuts and enablement","read_only":true},
        {"name":"prefs.execute","args":{"id":"string"},"description":"Run one app command by id, as its menu item, button or shortcut would (file.quit, view.refresh, view.panel.applications, apps.check, apps.update_all, apps.install, apps.rollback, apps.remove, help.shortcuts, help.about); the operation it starts is visible in prefs.info","read_only":false},
        {"name":"prefs.quit","description":"Close once the operation in progress finishes","read_only":false},
        {"name":"prefs.panel","args":{"name":"\"applications\""},"description":"Show a panel","read_only":false},
        {"name":"prefs.select","args":{"app":"an app name from prefs.apps"},"description":"Select an app in the Applications panel and load its release notes","read_only":false},
        {"name":"prefs.apps","description":"The Applications panel's rows, selection and notes","read_only":true},
        {"name":"prefs.dialog","args":{"open":"\"about\", \"shortcuts\" or null"},"description":"Open a dialog, or close the open one (a remove confirmation is closed, never confirmed, here)","read_only":false},
        {"name":"prefs.theme","args":{"scheme":"optional scheme name or null to follow the session","mode":"optional light, dark or null to follow the session"},"description":"Choose the window's theme: a present key sets that axis, an absent one leaves it; answers the choice and the effective scheme and mode","read_only":false}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_verb_is_described_once() {
        let described: Vec<_> = describe()["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, VERBS);
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
            installed: Some("1.0".into()),
            latest: None,
            published: None,
            status: "unchecked".into(),
        };
        assert_eq!(row.state(), RowState::Current);
        assert!(!row.installable());
    }

    #[test]
    fn panels_round_trip_by_name() {
        for panel in Panel::ALL {
            assert_eq!(Panel::from_name(panel.name()), Some(panel));
        }
        assert_eq!(Panel::from_name("nope"), None);
    }
}
