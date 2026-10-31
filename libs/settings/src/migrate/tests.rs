// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use crate::resolve::values as resolver;
use crate::{AppOverride, Panel};
use serde_json::json;

/// Migrates `desktop`, checking that the result passes the registry.
fn migrated(desktop: &Desktop) -> Migrated {
    let migrated = migrate(desktop).unwrap();
    registry::validate(&migrated.values).unwrap();
    migrated
}

/// A default record with one change, migrated.
fn with(change: impl FnOnce(&mut Desktop)) -> Migrated {
    let mut desktop = Desktop::default();
    change(&mut desktop);
    migrated(&desktop)
}

/// Migrates a parsed backdrop document, checking that the result passes the
/// registry.
fn backdrop(document: Value) -> Migrated {
    let migrated = migrate_backdrop(&document).unwrap();
    registry::validate(&migrated.values).unwrap();
    migrated
}

fn stored<'a>(migrated: &'a Migrated, path: &str) -> Option<&'a Value> {
    migrated.values.get(path)
}

#[test]
fn roundness_from_each_style() {
    let cases = [
        ("studio", None),
        ("adwaita", None),
        ("breeze", Some("slightly_round")),
        ("pro", Some("slightly_round")),
        ("classic", Some("square")),
        ("plain", Some("square")),
    ];
    for (style, roundness) in cases {
        let m = with(|d| d.appearance.style = Some(style.into()));
        assert_eq!(
            stored(&m, "appearance.roundness"),
            roundness.map(|r| json!(r)).as_ref(),
            "style {style}"
        );
        assert_eq!(stored(&m, "appearance.style"), Some(&json!(style)));
    }
}

#[test]
fn roundness_from_a_null_style_takes_the_schemes_own_style() {
    let cases = [
        ("studio", None),
        ("mixos", None),
        ("adwaita", None),
        ("pro", Some("slightly_round")),
        ("breeze", Some("slightly_round")),
        ("classic", Some("square")),
        ("forest", Some("square")),
        ("mono", Some("square")),
    ];
    for (scheme, roundness) in cases {
        let m = with(|d| d.appearance.scheme = scheme.into());
        assert_eq!(
            stored(&m, "appearance.roundness"),
            roundness.map(|r| json!(r)).as_ref(),
            "scheme {scheme}"
        );
        assert_eq!(stored(&m, "appearance.style"), None, "scheme {scheme}");
    }
}

#[test]
fn density_on_the_three_steps_maps_exactly_with_no_report() {
    for (density, name) in [
        (0.85, Some("compact")),
        (1.0, None),
        (1.2, Some("spacious")),
    ] {
        let m = with(|d| d.ui.density = density);
        assert_eq!(
            stored(&m, "appearance.density"),
            name.map(|n| json!(n)).as_ref(),
            "density {density}"
        );
        assert!(m.report.changed.is_empty(), "density {density}");
    }
}

#[test]
fn off_step_densities_map_by_the_midpoints_and_are_reported() {
    let cases = [
        (0.5, "compact"),
        (0.92, "compact"),
        (0.93, "comfortable"),
        (1.09, "comfortable"),
        (1.11, "spacious"),
        (2.0, "spacious"),
    ];
    for (density, name) in cases {
        let m = with(|d| d.ui.density = density);
        assert_eq!(
            stored(&m, "appearance.density"),
            (name != "comfortable").then(|| json!(name)).as_ref(),
            "density {density}"
        );
        assert_eq!(m.report.changed.len(), 1, "density {density}");
        assert_eq!(m.report.changed[0].from, "ui.density");
        assert_eq!(m.report.changed[0].old, json!(density));
        assert_eq!(m.report.changed[0].to.as_deref(), Some("appearance.density"));
    }
}

#[test]
fn panel_dock_stores_no_mode_and_no_autohide() {
    let m = with(|_| {});
    assert_eq!(stored(&m, "shell.panels.bottom.mode"), None);
    assert_eq!(stored(&m, "shell.panels.bottom.autohide"), None);
}

#[test]
fn panel_overlay_stores_its_mode() {
    let m = with(|d| d.shell.panels.get_mut("bottom").unwrap().mode = "overlay".into());
    assert_eq!(stored(&m, "shell.panels.bottom.mode"), Some(&json!("overlay")));
    assert_eq!(stored(&m, "shell.panels.bottom.autohide"), None);
}

#[test]
fn panel_hidden_becomes_dock_with_autohide() {
    let m = with(|d| d.shell.panels.get_mut("bottom").unwrap().mode = "hidden".into());
    assert_eq!(stored(&m, "shell.panels.bottom.mode"), None);
    assert_eq!(stored(&m, "shell.panels.bottom.autohide"), Some(&json!(true)));
}

#[test]
fn panel_thickness_snaps_to_the_nearest_step_and_a_tie_goes_up() {
    for (thickness, step) in [(16, 32), (36, 40), (48, 56), (40, 40), (256, 80)] {
        let m = with(|d| d.shell.panels.get_mut("bottom").unwrap().thickness = thickness);
        assert_eq!(
            registry::effective(&m.values, "shell.panels.bottom.thickness"),
            Some(json!(step)),
            "thickness {thickness}"
        );
        assert_eq!(m.report.changed.is_empty(), thickness == step);
    }
}

#[test]
fn an_unknown_panel_id_is_dropped_and_reported() {
    let m = with(|d| {
        d.shell.panels.insert("left".into(), Panel::default());
    });
    assert!(m.values.keys().all(|k| !k.starts_with("shell.panels.left")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].from, "shell.panels.left");
}

#[test]
fn shell_page_order_is_dropped_and_reported() {
    let m = with(|d| d.shell.page_order = vec!["workspaces".into()]);
    assert!(m.values.keys().all(|k| !k.contains("page_order")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].from, "shell.page_order");
    assert_eq!(m.report.dropped[0].old, json!(["workspaces"]));
}

#[test]
fn the_apps_record_is_flattened() {
    let m = with(|d| {
        d.apps.insert(
            "term".into(),
            AppOverride {
                scheme: Some("ocean".into()),
                mode: Some("light".into()),
                contrast: Some("high".into()),
                text_scale: Some(1.5),
            },
        );
        d.apps.insert(
            "ced".into(),
            AppOverride {
                mode: Some("light".into()),
                ..AppOverride::default()
            },
        );
    });
    assert_eq!(stored(&m, "apps.term.scheme"), Some(&json!("ocean")));
    assert_eq!(stored(&m, "apps.term.mode"), Some(&json!("light")));
    assert_eq!(stored(&m, "apps.term.contrast"), Some(&json!("high")));
    assert_eq!(stored(&m, "apps.term.text_scale"), Some(&json!(1.5)));
    assert_eq!(stored(&m, "apps.ced.mode"), Some(&json!("light")));
    assert_eq!(stored(&m, "apps.ced.scheme"), None);
}

#[test]
fn keymaps_and_the_app_domains_are_copied_leaf_by_leaf() {
    let m = with(|d| {
        d.keymaps.insert(
            "dopus".into(),
            [("nav.parent".into(), vec!["Ctrl+U".into()])].into(),
        );
        d.terminal.font.size = 13.5;
        d.terminal.padding = 2;
        d.email.mail.page_size = 75;
        d.ced.editor.padding = 4;
    });
    assert_eq!(
        stored(&m, "keymaps.dopus.nav.parent"),
        Some(&json!(["Ctrl+U"]))
    );
    assert_eq!(stored(&m, "terminal.font.size"), Some(&json!(13.5)));
    assert_eq!(stored(&m, "terminal.padding"), Some(&json!(2)));
    assert_eq!(stored(&m, "email.mail.page_size"), Some(&json!(75)));
    assert_eq!(stored(&m, "ced.editor.padding"), Some(&json!(4)));
    assert!(m.report.dropped.is_empty());
}

#[test]
fn an_empty_record_keeps_only_its_scheme() {
    // The format-4 record always serialises `scheme` (model.rs), and today's
    // default scheme is studio, not the v2 default mixos (§2.2): so the one
    // value an empty record carries is that scheme, copied explicitly.
    let m = migrated(&Desktop::default());
    assert_eq!(m.values.len(), 1, "{:?}", m.values);
    assert_eq!(stored(&m, "appearance.scheme"), Some(&json!("studio")));
    assert!(m.report.dropped.is_empty());
    assert!(m.report.changed.is_empty());
}

#[test]
fn a_chord_bound_twice_is_reported_not_refused() {
    let m = with(|d| {
        d.keymaps.insert(
            "dopus".into(),
            [
                ("nav.up".into(), vec!["Ctrl+K".into()]),
                ("nav.down".into(), vec!["Ctrl+K".into()]),
            ]
            .into(),
        );
    });
    assert_eq!(m.report.conflicts.len(), 1);
    assert_eq!(m.report.conflicts[0].code, "constraint_failed");
}

#[test]
fn backdrop_default_only() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"image": "/home/user/walls/a.png", "fit": "stretch"}
    }));
    assert_eq!(
        stored(&m, "wallpaper.image"),
        Some(&json!("/home/user/walls/a.png"))
    );
    assert_eq!(stored(&m, "wallpaper.fit"), Some(&json!("stretch")));
    assert_eq!(stored(&m, "wallpaper.same_on_all"), None);
    assert!(m.report.dropped.is_empty());
}

#[test]
fn backdrop_default_plus_outputs_sets_same_on_all_false_and_reports_what_it_drops() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"colour": "#1e1e2e"},
        "outputs": {
            "DP-1": {
                "image": "/home/user/walls/hills.jpg",
                "colour": "#000000",
                "fit": "fit",
                "slideshow": {"folder": "/home/user/walls"}
            },
            "DP-2": {"colour": "#112233", "fallback": "#000000"}
        }
    }));
    assert_eq!(stored(&m, "wallpaper.color"), Some(&json!("#1e1e2e")));
    assert_eq!(stored(&m, "wallpaper.same_on_all"), Some(&json!(false)));
    assert_eq!(
        stored(&m, "wallpaper.outputs.DP-1.image"),
        Some(&json!("/home/user/walls/hills.jpg"))
    );
    assert_eq!(stored(&m, "wallpaper.outputs.DP-2.image"), None);
    let dropped: Vec<&str> = m.report.dropped.iter().map(|e| e.from.as_str()).collect();
    assert_eq!(
        dropped,
        [
            "backdrop.outputs.DP-1.colour",
            "backdrop.outputs.DP-1.fit",
            "backdrop.outputs.DP-1.slideshow",
            "backdrop.outputs.DP-2.colour",
            "backdrop.outputs.DP-2.fallback",
        ]
    );
}

#[test]
fn backdrop_slideshow_maps_to_the_folder_form() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"slideshow": {"folder": "/home/user/walls", "interval_s": 300, "shuffle": true}}
    }));
    assert_eq!(stored(&m, "wallpaper.slideshow.enabled"), Some(&json!(true)));
    assert_eq!(
        stored(&m, "wallpaper.slideshow.folder"),
        Some(&json!("/home/user/walls"))
    );
    assert_eq!(stored(&m, "wallpaper.slideshow.interval_s"), Some(&json!(300)));
    assert_eq!(stored(&m, "wallpaper.slideshow.shuffle"), Some(&json!(true)));
    assert!(m.report.dropped.is_empty());
}

#[test]
fn backdrop_slideshow_of_images_without_a_folder_is_dropped() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"slideshow": {"images": ["/home/user/walls/a.png"]}}
    }));
    assert!(m.values.keys().all(|k| !k.starts_with("wallpaper.slideshow")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].from, "backdrop.default.slideshow");
}

#[test]
fn backdrop_centre_and_tile_fits_are_dropped() {
    for fit in ["centre", "tile"] {
        let m = backdrop(json!({
            "schema": 1,
            "default": {"image": "/home/user/walls/a.png", "fit": fit}
        }));
        assert_eq!(stored(&m, "wallpaper.fit"), None, "fit {fit}");
        assert_eq!(m.report.dropped.len(), 1, "fit {fit}");
        assert_eq!(m.report.dropped[0].from, "backdrop.default.fit");
    }
}

#[test]
fn a_backdrop_document_of_another_schema_is_refused() {
    assert!(migrate_backdrop(&json!({"schema": 2})).is_err());
}

/// A rich record and its backdrop: every look field, a non-default value
/// in each app domain, and per-output images.
#[test]
fn a_rich_record_and_its_backdrop_validate_and_resolve() {
    let mut desktop = Desktop::default();
    desktop.appearance.scheme = "forest".into();
    desktop.appearance.style = Some("classic".into());
    desktop.appearance.contrast = "high".into();
    desktop.appearance.window_style = "win11".into();
    desktop.appearance.decorations = "ssd".into();
    desktop.appearance.caption_side = "left".into();
    desktop.appearance.menu_style = "button".into();
    desktop.appearance.menu_side = "right".into();
    desktop.appearance.actions_place = "titlebar".into();
    desktop.appearance.actions_align = "left".into();
    desktop.appearance.title_align = "left".into();
    desktop.appearance.font_family = Some("Noto Sans".into());
    desktop.appearance.font_size = Some(16.0);
    desktop.appearance.font_weight = Some(300);
    desktop.appearance.mono_family = Some("Hack".into());
    desktop.appearance.icon_stroke = Some(1.75);
    desktop.ui.density = 0.85;
    desktop.ui.text_scale = 1.25;
    desktop.ui.reduced_motion = true;
    desktop.shell.panels.get_mut("bottom").unwrap().thickness = 56;
    desktop.shell.panels.get_mut("bottom").unwrap().mode = "overlay".into();
    desktop.keymaps.insert(
        "dopus".into(),
        [("nav.parent".into(), vec!["Ctrl+U".into()])].into(),
    );
    desktop.terminal.font.size = 13.5;
    desktop.ced.editor.padding = 4;
    desktop.email.mail.page_size = 75;
    desktop.apps.insert(
        "term".into(),
        AppOverride {
            scheme: Some("ocean".into()),
            contrast: Some("normal".into()),
            text_scale: Some(1.5),
            ..AppOverride::default()
        },
    );
    let mut result = migrated(&desktop);
    let walls = backdrop(json!({
        "schema": 1,
        "default": {"colour": "#1e1e2e"},
        "outputs": {"DP-1": {"image": "/home/user/walls/hills.jpg"}}
    }));
    result.values.extend(walls.values);
    registry::validate(&result.values).unwrap();
    resolver::resolve(&result.values, design::EMBEDDED_DEFAULT_SOURCE).unwrap();
}
