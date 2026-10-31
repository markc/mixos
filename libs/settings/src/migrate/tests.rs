// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use crate::resolve::values as resolver;
use crate::{AppOverride, Panel};
use serde_json::json;

/// Migrates `desktop`, checking that the result passes the registry. The
/// record must first pass the frozen v1 validator: a record v1 refuses is
/// not a format-4 record, so it is never a test input.
fn migrated(desktop: &Desktop) -> Migrated {
    crate::v1::resolve(desktop).unwrap();
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

/// A record whose `shell.panels` are exactly `panels`, migrated.
fn panels_with(panels: &[(&str, Panel)]) -> Migrated {
    with(|d| {
        d.shell.panels = panels
            .iter()
            .map(|(id, panel)| ((*id).into(), panel.clone()))
            .collect();
    })
}

fn panel(edge: &str) -> Panel {
    Panel {
        edge: edge.into(),
        ..Panel::default()
    }
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

/// The rich record: every look field, a non-default value in each app
/// domain, and one app override.
fn rich_desktop() -> Desktop {
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
    desktop
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
        ("adwaita", None),
        ("pro", Some("slightly_round")),
        ("breeze", Some("slightly_round")),
        ("classic", Some("square")),
        ("ocean", Some("square")),
        ("crimson", Some("square")),
        ("stone", Some("square")),
        ("forest", Some("square")),
        ("sunset", Some("square")),
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

/// The table is the embedded design's own binding, read from the design, so
/// it cannot drift from it. Only the authored schemes: format 4 cannot hold
/// the derived `mixos`, so a migrated record never names it.
#[test]
fn scheme_style_follows_the_embedded_binding() {
    let doc = design::parse_design_source(
        design::SourceIdentity::new("embedded"),
        design::EMBEDDED_DEFAULT_SOURCE,
    )
    .unwrap();
    let binding = &doc.v1.families.style.as_ref().unwrap().schemes;
    for scheme in design::Scheme::AUTHORED {
        let name = scheme.name();
        assert_eq!(
            scheme_style(name),
            binding.get(name).map(String::as_str),
            "scheme {name}"
        );
    }
}

#[test]
fn a_pinned_source_with_a_null_style_is_reported() {
    let m = with(|d| d.appearance.source = Some(design::EMBEDDED_DEFAULT_SOURCE.into()));
    assert_eq!(m.report.changed.len(), 1);
    assert_eq!(m.report.changed[0].code, EMBEDDED_BINDING);
    assert_eq!(m.report.changed[0].from, "appearance.style");
    assert_eq!(
        m.report.changed[0].to.as_deref(),
        Some("appearance.roundness")
    );
    assert!(stored(&m, "appearance.source").is_some());
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
        (0.9249, "compact"),
        (0.92, "compact"),
        (0.925, "comfortable"),
        (0.93, "comfortable"),
        (1.09, "comfortable"),
        (1.1, "comfortable"),
        (1.1001, "spacious"),
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
        assert_eq!(m.report.changed[0].code, OFF_STEP);
        assert_eq!(m.report.changed[0].from, "ui.density");
        assert_eq!(m.report.changed[0].old, json!(density));
        assert_eq!(
            m.report.changed[0].to.as_deref(),
            Some("appearance.density")
        );
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
    assert_eq!(
        stored(&m, "shell.panels.bottom.mode"),
        Some(&json!("overlay"))
    );
    assert_eq!(stored(&m, "shell.panels.bottom.autohide"), None);
}

#[test]
fn panel_hidden_becomes_dock_with_autohide() {
    let m = with(|d| d.shell.panels.get_mut("bottom").unwrap().mode = "hidden".into());
    assert_eq!(stored(&m, "shell.panels.bottom.mode"), None);
    assert_eq!(
        stored(&m, "shell.panels.bottom.autohide"),
        Some(&json!(true))
    );
}

#[test]
fn panel_thickness_snaps_to_the_nearest_step_and_a_tie_goes_up() {
    for (thickness, step) in [
        (16, 32),
        (36, 40),
        (48, 56),
        (60, 64),
        (72, 80),
        (40, 40),
        (256, 80),
    ] {
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
fn a_snapped_thickness_is_reported_with_its_fields() {
    let m = with(|d| d.shell.panels.get_mut("bottom").unwrap().thickness = 36);
    assert_eq!(m.report.changed.len(), 1);
    let entry = &m.report.changed[0];
    assert_eq!(entry.code, SNAPPED);
    assert_eq!(entry.from, "shell.panels.bottom.thickness");
    assert_eq!(entry.old, json!(36));
    assert_eq!(entry.to.as_deref(), Some("shell.panels.bottom.thickness"));
}

#[test]
fn a_top_panel_keeps_its_own_thickness_default() {
    for (thickness, stored_as) in [(32, None), (40, Some(40))] {
        let m = panels_with(&[
            (
                "top",
                Panel {
                    thickness,
                    ..panel("top")
                },
            ),
            ("bottom", panel("bottom")),
        ]);
        assert_eq!(
            stored(&m, "shell.panels.top.thickness"),
            stored_as.map(|t| json!(t)).as_ref(),
            "thickness {thickness}"
        );
        assert_eq!(
            registry::effective(&m.values, "shell.panels.top.thickness"),
            Some(json!(thickness))
        );
    }
}

#[test]
fn a_top_panel_alone_is_enabled_and_the_absent_bottom_is_off() {
    let m = panels_with(&[("top", panel("top"))]);
    assert_eq!(stored(&m, "shell.panels.top.enabled"), Some(&json!(true)));
    assert_eq!(
        stored(&m, "shell.panels.bottom.enabled"),
        Some(&json!(false))
    );
    assert_eq!(m.report.changed.len(), 1);
    assert_eq!(m.report.changed[0].code, BOTTOM_ABSENT);
}

#[test]
fn a_bottom_panel_present_keeps_the_default_enabled() {
    // Bottom is on by default, so the present panel stores nothing.
    let m = with(|_| {});
    assert_eq!(stored(&m, "shell.panels.bottom.enabled"), None);
    assert_eq!(stored(&m, "shell.panels.top.enabled"), None);
    assert!(m.report.changed.is_empty());
}

#[test]
fn both_panels_absent_switch_the_bottom_off_and_report_it() {
    let m = panels_with(&[]);
    assert_eq!(stored(&m, "shell.panels.top.enabled"), None);
    assert_eq!(
        stored(&m, "shell.panels.bottom.enabled"),
        Some(&json!(false))
    );
    assert_eq!(m.report.changed.len(), 1);
    assert_eq!(m.report.changed[0].code, BOTTOM_ABSENT);
    assert_eq!(m.report.changed[0].from, "shell.panels.bottom");
}

#[test]
fn both_panels_on_distinct_edges_are_both_enabled() {
    let m = panels_with(&[("top", panel("top")), ("bottom", panel("bottom"))]);
    assert_eq!(stored(&m, "shell.panels.top.enabled"), Some(&json!(true)));
    assert_eq!(stored(&m, "shell.panels.bottom.enabled"), None);
    assert!(m.report.changed.is_empty());
}

/// Format 4 never checked edges. The top panel on the bottom edge is switched
/// off, so the bottom keeps the edge and the record still validates.
#[test]
fn a_top_panel_on_the_bottom_edge_is_switched_off_and_reported() {
    let m = panels_with(&[
        (
            "top",
            Panel {
                mode: "overlay".into(),
                ..panel("bottom")
            },
        ),
        ("bottom", panel("bottom")),
    ]);
    assert_eq!(
        registry::effective(&m.values, "shell.panels.top.enabled"),
        Some(json!(false))
    );
    assert_eq!(stored(&m, "shell.panels.top.edge"), Some(&json!("bottom")));
    assert_eq!(stored(&m, "shell.panels.top.mode"), Some(&json!("overlay")));
    assert_eq!(stored(&m, "shell.panels.bottom.enabled"), None);
    assert_eq!(m.report.changed.len(), 1);
    assert_eq!(m.report.changed[0].code, SHARED_EDGE);
    assert_eq!(m.report.changed[0].from, "shell.panels.top.edge");
    assert_eq!(
        m.report.changed[0].to.as_deref(),
        Some("shell.panels.top.enabled")
    );
}

#[test]
fn an_unknown_panel_id_is_dropped_and_reported() {
    let m = with(|d| {
        d.shell.panels.insert("left".into(), panel("left"));
    });
    assert!(m.values.keys().all(|k| !k.starts_with("shell.panels.left")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].code, UNKNOWN_PANEL);
    assert_eq!(m.report.dropped[0].from, "shell.panels.left");
}

#[test]
fn shell_page_order_is_dropped_and_reported() {
    let m = with(|d| d.shell.page_order = vec!["workspaces".into()]);
    assert!(m.values.keys().all(|k| !k.contains("page_order")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].code, RETIRED);
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
fn an_app_override_with_nothing_set_stores_and_reports_nothing() {
    let m = with(|d| {
        d.apps.insert("ced".into(), AppOverride::default());
    });
    assert!(m.values.keys().all(|k| !k.starts_with("apps.")));
    assert!(m.report.dropped.is_empty());
    assert!(m.report.changed.is_empty());
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
fn an_empty_keymap_list_is_stored_as_unbound() {
    let m = with(|d| {
        d.keymaps
            .insert("dopus".into(), [("nav.parent".into(), Vec::new())].into());
    });
    assert_eq!(stored(&m, "keymaps.dopus.nav.parent"), Some(&json!([])));
}

#[test]
fn compositor_keymaps_are_dropped_whole_and_reported() {
    let m = with(|d| {
        d.keymaps.insert(
            "comp".into(),
            [("spawn.x".into(), vec!["Ctrl+Alt+T".into()])].into(),
        );
    });
    assert!(m.values.keys().all(|k| !k.starts_with("keymaps.comp")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].code, COMPD_KEYMAPS);
    assert_eq!(m.report.dropped[0].from, "keymaps.comp");
}

#[test]
fn email_accounts_are_one_array_leaf() {
    let account: crate::email::AccountEntry = serde_json::from_value(json!({
        "id": "acct-work",
        "name": "Work",
        "url": "https://mail.example.org",
        "email": "me@example.org"
    }))
    .unwrap();
    let m = with(|d| d.email.accounts = vec![account]);
    assert_eq!(
        stored(&m, "email.accounts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert!(m.values.keys().all(|k| !k.starts_with("email.accounts.")));
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
fn the_rich_record_maps_every_row_exactly() {
    let m = migrated(&rich_desktop());
    let expected: Values = [
        ("appearance.scheme", json!("forest")),
        ("appearance.style", json!("classic")),
        ("appearance.roundness", json!("square")),
        ("appearance.contrast", json!("high")),
        ("window.style", json!("win11")),
        ("window.decorations", json!("ssd")),
        ("window.caption_side", json!("left")),
        ("window.menu_style", json!("button")),
        ("window.menu_side", json!("right")),
        ("window.actions_place", json!("titlebar")),
        ("window.actions_align", json!("left")),
        ("window.title_align", json!("left")),
        ("appearance.icon_stroke", json!(1.75)),
        ("appearance.font_family", json!("Noto Sans")),
        ("appearance.font_weight", json!(300)),
        ("appearance.font_size", json!(16.0)),
        ("appearance.mono_family", json!("Hack")),
        ("appearance.density", json!("compact")),
        ("accessibility.text_scale", json!(1.25)),
        ("accessibility.reduced_motion", json!(true)),
        ("shell.panels.bottom.mode", json!("overlay")),
        ("shell.panels.bottom.thickness", json!(56)),
        ("keymaps.dopus.nav.parent", json!(["Ctrl+U"])),
        ("terminal.font.size", json!(13.5)),
        ("ced.editor.padding", json!(4)),
        ("email.mail.page_size", json!(75)),
        ("apps.term.scheme", json!("ocean")),
        ("apps.term.contrast", json!("normal")),
        ("apps.term.text_scale", json!(1.5)),
    ]
    .into_iter()
    .map(|(path, value)| (path.to_owned(), value))
    .collect();
    assert_eq!(m.values, expected);
    assert!(m.report.dropped.is_empty());
    assert!(m.report.changed.is_empty());
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
fn a_default_fallback_is_dropped_and_reported() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"colour": "#1e1e2e", "fallback": "#000000"}
    }));
    assert_eq!(stored(&m, "wallpaper.color"), Some(&json!("#1e1e2e")));
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].code, NO_V2_KEY);
    assert_eq!(m.report.dropped[0].from, "backdrop.default.fallback");
}

#[test]
fn a_backdrop_colour_is_normalised_to_lower_case() {
    let m = backdrop(json!({"schema": 1, "default": {"colour": "#1E1E2E"}}));
    assert_eq!(stored(&m, "wallpaper.color"), Some(&json!("#1e1e2e")));
}

#[test]
fn backdrop_slideshow_interval_snaps_to_the_nearest_step_and_a_tie_goes_up() {
    // (asked, stored): 1350 is level between 900 and 1800, so it goes up. 450
    // snaps to 600, the registry default, which is not stored.
    let cases = [
        (100, Some(300)),
        (450, None),
        (1350, Some(1800)),
        (5000, Some(3600)),
        (10000, Some(7200)),
        (300, Some(300)),
    ];
    for (asked, stored_as) in cases {
        let m = backdrop(json!({
            "schema": 1,
            "default": {"slideshow": {"folder": "/home/user/walls", "interval_s": asked}}
        }));
        assert_eq!(
            stored(&m, "wallpaper.slideshow.interval_s"),
            stored_as.map(|s| json!(s)).as_ref(),
            "interval {asked}"
        );
        assert_eq!(
            m.report.changed.len(),
            usize::from(asked != 300),
            "interval {asked}"
        );
        if asked != 300 {
            assert_eq!(m.report.changed[0].code, SNAPPED);
            assert_eq!(
                m.report.changed[0].from,
                "backdrop.default.slideshow.interval_s"
            );
            assert_eq!(m.report.changed[0].old, json!(asked));
        }
    }
}

#[test]
fn backdrop_slideshow_maps_to_the_folder_form() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"slideshow": {"folder": "/home/user/walls", "interval_s": 300, "shuffle": true}}
    }));
    assert_eq!(
        stored(&m, "wallpaper.slideshow.enabled"),
        Some(&json!(true))
    );
    assert_eq!(
        stored(&m, "wallpaper.slideshow.folder"),
        Some(&json!("/home/user/walls"))
    );
    assert_eq!(
        stored(&m, "wallpaper.slideshow.interval_s"),
        Some(&json!(300))
    );
    assert_eq!(
        stored(&m, "wallpaper.slideshow.shuffle"),
        Some(&json!(true))
    );
    assert!(m.report.dropped.is_empty());
}

#[test]
fn backdrop_slideshow_of_images_without_a_folder_is_dropped() {
    let m = backdrop(json!({
        "schema": 1,
        "default": {"slideshow": {"images": ["/home/user/walls/a.png"]}}
    }));
    assert!(
        m.values
            .keys()
            .all(|k| !k.starts_with("wallpaper.slideshow"))
    );
    assert_eq!(m.report.dropped.len(), 1);
    assert_eq!(m.report.dropped[0].from, "backdrop.default.slideshow");
}

#[test]
fn a_slideshow_with_neither_folder_nor_images_is_refused() {
    let failed = migrate_backdrop(&json!({"schema": 1, "default": {"slideshow": {}}})).unwrap_err();
    assert_eq!(failed.errors.len(), 1);
    assert_eq!(failed.errors[0].path, "backdrop.default.slideshow");
}

#[test]
fn every_refusal_is_gathered_with_the_report() {
    let failed = migrate_backdrop(&json!({
        "schema": 1,
        "default": {"slideshow": {}, "fallback": "#000000"},
        "outputs": {
            "bad output": {"image": "/home/user/walls/a.png"},
            "also bad": {"image": "/home/user/walls/b.png"}
        }
    }))
    .unwrap_err();
    assert!(failed.errors.len() >= 3, "{:?}", failed.errors);
    assert_eq!(failed.report.dropped.len(), 1);
    assert_eq!(failed.report.dropped[0].from, "backdrop.default.fallback");
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

#[test]
fn an_import_onto_an_empty_store_is_the_backdrop_migration() {
    let doc = json!({
        "schema": 1,
        "default": {"image": "/home/user/walls/a.png"},
        "outputs": {"DP-1": {"image": "/home/user/walls/b.png"}}
    });
    let imported = import_backdrop(&Values::new(), &doc).unwrap();
    assert_eq!(imported.values, migrate_backdrop(&doc).unwrap().values);
}

#[test]
fn an_import_replaces_wallpaper_keys_and_keeps_the_rest() {
    let current: Values = [
        ("appearance.mode".to_owned(), json!("light")),
        (
            "wallpaper.image".to_owned(),
            json!("/home/user/walls/old.png"),
        ),
        ("wallpaper.same_on_all".to_owned(), json!(false)),
    ]
    .into_iter()
    .collect();
    let imported = import_backdrop(
        &current,
        &json!({"schema": 1, "default": {"colour": "#1e1e2e"}}),
    )
    .unwrap();
    let expected: Values = [
        ("appearance.mode".to_owned(), json!("light")),
        ("wallpaper.color".to_owned(), json!("#1e1e2e")),
    ]
    .into_iter()
    .collect();
    assert_eq!(imported.values, expected);
    let froms: Vec<&str> = imported
        .report
        .changed
        .iter()
        .map(|e| e.from.as_str())
        .collect();
    assert_eq!(froms, ["wallpaper.image", "wallpaper.same_on_all"]);
    assert!(imported.report.changed.iter().all(|e| e.code == CLEARED));
}

#[test]
fn an_import_reports_a_replaced_value_and_not_an_unchanged_one() {
    let doc = json!({"schema": 1, "default": {"colour": "#1e1e2e"}});
    let current: Values = [("wallpaper.color".to_owned(), json!("#000000"))]
        .into_iter()
        .collect();
    let imported = import_backdrop(&current, &doc).unwrap();
    assert_eq!(imported.report.changed.len(), 1);
    assert_eq!(imported.report.changed[0].code, REPLACED);
    assert_eq!(imported.report.changed[0].old, json!("#000000"));
    assert_eq!(
        imported.report.changed[0].to.as_deref(),
        Some("wallpaper.color")
    );
    let same: Values = [("wallpaper.color".to_owned(), json!("#1e1e2e"))]
        .into_iter()
        .collect();
    assert!(
        import_backdrop(&same, &doc)
            .unwrap()
            .report
            .changed
            .is_empty()
    );
}

#[test]
fn an_import_validates_the_union() {
    // With the only output switched off, C6 refuses the store, and the import
    // with it.
    let current: Values = [("displays.DP-1.enabled".to_owned(), json!(false))]
        .into_iter()
        .collect();
    assert!(import_backdrop(&current, &json!({"schema": 1})).is_err());
}

/// The rich record and its backdrop, merged, validate and resolve.
#[test]
fn a_rich_record_and_its_backdrop_validate_and_resolve() {
    let mut result = migrated(&rich_desktop());
    let walls = backdrop(json!({
        "schema": 1,
        "default": {"colour": "#1e1e2e"},
        "outputs": {"DP-1": {"image": "/home/user/walls/hills.jpg"}}
    }));
    result.values.extend(walls.values);
    registry::validate(&result.values).unwrap();
    resolver::resolve(&result.values, design::EMBEDDED_DEFAULT_SOURCE).unwrap();
}
