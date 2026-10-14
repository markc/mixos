// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The renderer-independent navigation model: the sections the sidebar
//! offers, which panel each belongs to, and which Applications rows sit
//! under each section.
use serde::{Deserialize, Serialize};

use crate::model::{AppRow, Panel};

/// A sidebar section, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Themes,
    Fonts,
    Wallpapers,
    #[serde(rename = "mixos-desktop")]
    Desktop,
    #[serde(rename = "mixos-server")]
    Server,
    Creative,
    Other,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::Themes,
        Section::Fonts,
        Section::Wallpapers,
        Section::Desktop,
        Section::Server,
        Section::Creative,
        Section::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Section::Themes => "themes",
            Section::Fonts => "fonts",
            Section::Wallpapers => "wallpapers",
            Section::Desktop => "mixos-desktop",
            Section::Server => "mixos-server",
            Section::Creative => "creative",
            Section::Other => "other",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The panel that shows this section.
    pub fn panel(self) -> Panel {
        match self {
            Section::Themes | Section::Fonts | Section::Wallpapers => Panel::Appearance,
            _ => Panel::Applications,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Themes => "section-themes",
            Section::Fonts => "section-fonts",
            Section::Wallpapers => "section-wallpapers",
            Section::Desktop => "section-desktop",
            Section::Server => "section-server",
            Section::Creative => "section-creative",
            Section::Other => "section-other",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Section::Themes => "view.section.themes",
            Section::Fonts => "view.section.fonts",
            Section::Wallpapers => "view.section.wallpapers",
            Section::Desktop => "view.section.desktop",
            Section::Server => "view.section.server",
            Section::Creative => "view.section.creative",
            Section::Other => "view.section.other",
        }
    }

    /// The section a panel opens on.
    pub fn default_for(panel: Panel) -> Self {
        match panel {
            Panel::Appearance => Section::Themes,
            Panel::Applications => Section::Creative,
        }
    }

    /// Whether an Applications row sits under this section.
    pub fn matches(&self, row: &AppRow) -> bool {
        application_section(row) == *self
    }
}

/// Catalogue metadata owns grouping. Legacy MixOS bundle identifiers retain
/// their groups; unrecognised rows stay accessible under Other.
pub fn application_section(row: &AppRow) -> Section {
    if let Some(category) = row.category.as_deref() {
        return Section::from_name(category).filter(|s| s.panel() == Panel::Applications).unwrap_or(Section::Other);
    }
    match row.app.as_str() {
        "mixos-desktop" => Section::Desktop,
        "mixos-server" => Section::Server,
        _ => Section::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(app: &str, repo: &str) -> AppRow {
        AppRow {
            app: app.into(),
            repo: repo.into(),
            category: None,
            category_label: None,
            installed: None,
            latest: None,
            published: None,
            status: "unchecked".into(),
        }
    }

    #[test]
    fn every_section_round_trips_by_name_and_serde() {
        for section in Section::ALL {
            assert_eq!(Section::from_name(section.name()), Some(section));
            let json = serde_json::to_string(&section).unwrap();
            assert_eq!(json, format!("\"{}\"", section.name()));
            assert_eq!(serde_json::from_str::<Section>(&json).unwrap(), section);
        }
        assert_eq!(Section::from_name("nope"), None);
    }

    #[test]
    fn sections_map_to_their_panel_and_default() {
        assert_eq!(Section::Fonts.panel(), Panel::Appearance);
        assert_eq!(Section::Server.panel(), Panel::Applications);
        assert_eq!(Section::default_for(Panel::Appearance), Section::Themes);
        assert_eq!(Section::default_for(Panel::Applications), Section::Creative);
    }

    #[test]
    fn legacy_mixos_bundles_and_catalogue_categories_route() {
        assert_eq!(
            application_section(&row("mixos-desktop", "mixos/desktop")),
            Section::Desktop
        );
        assert_eq!(
            application_section(&row("mixos-server", "mixos/server")),
            Section::Server
        );
        let mut custom = row("canvas", "example/canvas");
        custom.category = Some("creative".into());
        assert_eq!(application_section(&custom), Section::Creative);
        custom.category = Some("mixos-desktop".into());
        assert_eq!(application_section(&custom), Section::Desktop);
    }

    #[test]
    fn unclassified_and_unknown_categories_stay_accessible() {
        assert_eq!(
            application_section(&row("someapp", "vendor/someapp")),
            Section::Other
        );
        assert!(Section::Other.matches(&row("someapp", "vendor/someapp")));
        let mut custom = row("canvas", "example/canvas");
        for category in ["themes", "unknown"] {
            custom.category = Some(category.into());
            assert_eq!(application_section(&custom), Section::Other);
        }
    }
}
