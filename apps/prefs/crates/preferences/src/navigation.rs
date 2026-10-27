// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The section registry owns navigation, commands and search in tree order.
use serde::{Deserialize, Serialize};

use crate::{Engine, model::AppRow};

macro_rules! categories {
    ($($variant:ident => $id:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Category {
            $(#[serde(rename = $id)] $variant),+
        }

        impl Category {
            pub const ALL: [Self; 7] = [$(Self::$variant),+];

            pub fn name(self) -> &'static str {
                match self { $(Self::$variant => $id),+ }
            }

            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => concat!("category-", $id)),+ }
            }
        }
    };
}

categories! {
    Overview => "overview",
    Appearance => "appearance",
    Desktop => "desktop",
    Input => "input",
    Output => "output",
    Apps => "apps",
    System => "system",
}

/// One editor's identity and searchable metadata. Rendering dispatches on
/// `section`; command handlers and all public listings come from this table.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SectionDescriptor {
    pub section: Section,
    pub id: &'static str,
    pub category: Category,
    pub title: &'static str,
    pub aliases: &'static [&'static str],
    pub prefixes: &'static [&'static str],
    pub command: &'static str,
    #[serde(skip)]
    pub run: fn(&mut Engine),
}

macro_rules! sections {
    ($($variant:ident => ($id:literal, $category:ident, $title:literal,
        $aliases:expr, $prefixes:expr)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Section {
            $(#[serde(rename = $id)] $variant),+
        }

        pub static SECTIONS: &[SectionDescriptor] = &[
            $(SectionDescriptor {
                section: Section::$variant,
                id: $id,
                category: Category::$category,
                title: $title,
                aliases: $aliases,
                prefixes: $prefixes,
                command: concat!("view.section.", $id),
                run: |e| e.set_section(Section::$variant),
            }),+
        ];
    };
}

sections! {
    Appearance => ("appearance", Appearance, "section-appearance", &["themes", "colour", "style", "density", "spacing", "compact"], &["appearance.scheme", "appearance.style", "appearance.mode", "appearance.contrast", "ui.density"]),
    TextAndFonts => ("text_and_fonts", Appearance, "section-text-and-fonts", &["typeface", "typography", "size"], &["appearance.font", "appearance.mono_family"]),
    Wallpapers => ("wallpaper", Appearance, "section-wallpapers", &["wallpapers", "background"], &[]),
    Windows => ("windows", Desktop, "section-windows", &["title bars", "window buttons", "window style", "menus", "hamburger"], &["appearance.decorations", "appearance.caption_side", "appearance.window_style", "appearance.menu_style", "appearance.menu_side"]),
    Email => ("apps_email", Apps, "section-email", &["mail", "accounts", "reader", "composer"], &["email."]),
    Terminal => ("apps_term", Apps, "nav-terminal", &["shell", "console"], &["terminal."]),
    Ced => ("apps_ced", Apps, "section-ced", &["editor"], &["ced.editor."]),
    Accessibility => ("accessibility", System, "section-accessibility", &["size", "animation", "reduce motion", "scale", "zoom", "larger text"], &["ui.reduced_motion", "ui.text_scale"]),
    Software => ("software", System, "section-software", &["system", "install", "update", "applications", "packages"], &[]),
}

impl Section {
    pub fn descriptor(self) -> &'static SectionDescriptor {
        SECTIONS
            .iter()
            .find(|s| s.section == self)
            .expect("registered section")
    }

    pub fn name(self) -> &'static str {
        self.descriptor().id
    }

    pub fn from_name(name: &str) -> Option<Self> {
        SECTIONS.iter().find(|s| s.id == name).map(|s| s.section)
    }

    pub fn category(self) -> Category {
        self.descriptor().category
    }

    pub fn label(self) -> &'static str {
        self.descriptor().title
    }

    pub fn command(self) -> &'static str {
        self.descriptor().command
    }
}

/// Match translated titles and category labels alongside aliases and keys.
/// Filtering preserves category and section order, except that an exact
/// "system" query ranks Software & Updates before Accessibility.
pub fn search(query: &str, label: impl Fn(&str) -> String) -> Vec<&'static SectionDescriptor> {
    let query = query.to_lowercase();
    let mut matches: Vec<_> = SECTIONS
        .iter()
        .filter(|s| {
            [label(s.title), label(s.category.label())]
                .iter()
                .map(String::as_str)
                .chain([s.id, s.category.name()])
                .chain(s.aliases.iter().copied())
                .chain(s.prefixes.iter().copied())
                .any(|text| text.to_lowercase().contains(&query))
                || s.prefixes.iter().any(|prefix| query.starts_with(prefix))
        })
        .collect();
    if query == "system" {
        matches.sort_by_key(|s| s.section != Section::Software);
    }
    matches
}

/// Software's catalogue groups are independent of sidebar sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoftwareGroup {
    #[serde(rename = "mixos-desktop")]
    Desktop,
    #[serde(rename = "mixos-server")]
    Server,
    #[serde(rename = "creative")]
    Creative,
    #[serde(rename = "other")]
    Other,
}

impl SoftwareGroup {
    pub const ALL: [Self; 4] = [Self::Desktop, Self::Server, Self::Creative, Self::Other];

    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "mixos-desktop",
            Self::Server => "mixos-server",
            Self::Creative => "creative",
            Self::Other => "other",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.name() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "group-desktop",
            Self::Server => "group-server",
            Self::Creative => "group-creative",
            Self::Other => "group-other",
        }
    }

    pub fn matches(self, row: &AppRow) -> bool {
        application_group(row) == self
    }
}

/// Catalogue metadata owns grouping. Unrecognised rows remain accessible.
pub fn application_group(row: &AppRow) -> SoftwareGroup {
    if let Some(category) = row.category.as_deref() {
        return SoftwareGroup::from_name(category).unwrap_or(SoftwareGroup::Other);
    }
    match row.app.as_str() {
        "mixos-desktop" => SoftwareGroup::Desktop,
        "mixos-server" => SoftwareGroup::Server,
        _ => SoftwareGroup::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(app: &str) -> AppRow {
        AppRow {
            app: app.into(),
            repo: "example/app".into(),
            category: None,
            category_label: None,
            installed: None,
            latest: None,
            published: None,
            status: "unchecked".into(),
        }
    }

    #[test]
    fn registry_ids_are_unique_and_round_trip() {
        let mut ids = std::collections::HashSet::new();
        for s in SECTIONS {
            assert!(ids.insert(s.id));
            assert_eq!(Section::from_name(s.id), Some(s.section));
            let json = serde_json::to_string(&s.section).unwrap();
            assert_eq!(json, format!("\"{}\"", s.id));
            assert_eq!(serde_json::from_str::<Section>(&json).unwrap(), s.section);
        }
        for id in [
            "mixos-desktop",
            "mixos-server",
            "creative",
            "other",
            "unknown",
            "themes",
            "fonts",
            "size",
            "motion",
            "text",
            "email",
            "terminal",
            "wallpapers",
        ] {
            assert_eq!(Section::from_name(id), None);
        }
    }

    #[test]
    fn search_matches_keys_aliases_and_category_labels_in_tree_order() {
        let ids = |query| {
            search(query, str::to_owned)
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("TERMINAL."), ["apps_term"]);
        assert_eq!(ids("background"), ["wallpaper"]);
        assert_eq!(
            ids("category-apps"),
            ["apps_email", "apps_term", "apps_ced"]
        );
        assert_eq!(ids("email.reader"), ["apps_email"]);
        assert_eq!(ids("ced.editor"), ["apps_ced"]);
        assert_eq!(ids("appearance.decorations"), ["windows"]);
        assert_eq!(ids("appearance.caption_side"), ["windows"]);
        assert_eq!(ids("appearance.window_style"), ["windows"]);
        assert_eq!(ids("ui.density"), ["appearance"]);
        assert_eq!(ids("compact"), ["appearance"]);
        assert_eq!(ids("size"), ["text_and_fonts", "accessibility"]);
        assert_eq!(ids("density"), ["appearance"]);
        assert_eq!(ids("themes"), ["appearance"]);
        assert_eq!(ids("wallpapers"), ["wallpaper"]);
        assert_eq!(ids("system")[0], "software");
        assert_eq!(
            &ids("appearance")[..3],
            ["appearance", "text_and_fonts", "wallpaper"]
        );
        assert_eq!(ids("ui.text_scale"), ["accessibility"]);
        assert_eq!(ids("ui.reduced_motion"), ["accessibility"]);
        assert_eq!(ids("animation"), ["accessibility"]);
        assert_eq!(ids("appearance.mono_family"), ["text_and_fonts"]);
        assert!(ids("no such setting").is_empty());
        assert_eq!(ids("").len(), SECTIONS.len());
    }

    #[test]
    fn catalogue_and_bundle_groups_route_without_becoming_sections() {
        assert_eq!(
            application_group(&row("mixos-desktop")),
            SoftwareGroup::Desktop
        );
        assert_eq!(
            application_group(&row("mixos-server")),
            SoftwareGroup::Server
        );
        let mut custom = row("canvas");
        assert_eq!(application_group(&custom), SoftwareGroup::Other);
        custom.category = Some("creative".into());
        assert_eq!(application_group(&custom), SoftwareGroup::Creative);
        custom.category = Some("unknown".into());
        assert_eq!(application_group(&custom), SoftwareGroup::Other);
    }
}
