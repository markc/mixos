// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use crate::{AppOverride, Desktop};
use serde_json::{Value, json};

fn values(changes: &[(&str, Value)]) -> Values {
    let changes = changes
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    registry::apply(&Values::new(), &changes, &[]).unwrap()
}

fn resolved(values: &Values) -> BTreeMap<String, Effective> {
    resolve(values, design::EMBEDDED_DEFAULT_SOURCE).unwrap().0
}

/// The mode-auto resolution table (ruling F11), row by row, through the
/// resolver and into every context.
#[test]
fn the_mode_auto_resolution_table() {
    let overrides = [None, Some("dark"), Some("light")];
    let daylight = [None, Some(true), Some(false)];
    for authored in ["dark", "light", "auto"] {
        for o in overrides {
            for d in daylight {
                let expected = match (authored, o, d) {
                    ("dark", _, _) => "dark",
                    ("light", _, _) => "light",
                    (_, Some(o), _) => o,
                    (_, None, Some(true)) => "light",
                    (_, None, _) => "dark",
                };
                let mut changes = vec![("appearance.mode", json!(authored))];
                if let Some(o) = o {
                    changes.push(("state.appearance.mode_override", json!(o)));
                }
                if let Some(d) = d {
                    changes.push(("state.appearance.daylight", json!(d)));
                }
                let effective = resolved(&values(&changes));
                let row = format!("{authored} / {o:?} / {d:?}");
                for (context, e) in &effective {
                    assert_eq!(e.mode, expected, "{row}: {context}");
                }
                let from = &effective["desktop"].provenance["appearance.mode"];
                let decided = match (authored, o, d) {
                    ("auto", Some(_), _) => "state.appearance.mode_override",
                    ("auto", None, Some(_)) => "state.appearance.daylight",
                    ("auto", None, None) => "auto_without_daylight",
                    _ => "profile",
                };
                assert_eq!(from, decided, "{row}");
            }
        }
    }
}

#[test]
fn an_app_mode_beats_the_resolved_auto_mode() {
    let effective = resolved(&values(&[
        ("appearance.mode", json!("auto")),
        ("state.appearance.daylight", json!(true)),
        ("apps.term.mode", json!("dark")),
    ]));
    assert_eq!(effective["desktop"].mode, "light");
    assert_eq!(effective["app:term"].mode, "dark");
    assert_eq!(effective["app:term"].provenance["appearance.mode"], "apps.term");
    assert_eq!(effective["app:ced"].mode, "light");
}

#[test]
fn an_invalid_record_is_refused_not_resolved_around() {
    let mut bad = Values::new();
    bad.insert("appearance.mode".into(), json!("dusk"));
    let errors = resolve(&bad, design::EMBEDDED_DEFAULT_SOURCE).unwrap_err();
    assert_eq!(errors[0].path, "appearance.mode");
    let mut unknown = Values::new();
    unknown.insert("ui.density".into(), json!(1.0));
    assert!(resolve(&unknown, design::EMBEDDED_DEFAULT_SOURCE).is_err());
}

/// Today's resolver and v2 agree on the same choices, field for field: the
/// look half starts from today's look. Provenance names the v2 keys
/// (`accessibility.text_scale` for `ui.text_scale`); nothing else differs.
#[test]
fn resolve_v2_reproduces_today_s_look_for_the_same_choices() {
    let mut desktop = Desktop::default();
    desktop.appearance.scheme = "forest".into();
    desktop.appearance.style = Some("classic".into());
    desktop.appearance.contrast = "high".into();
    desktop.appearance.window_style = "win11".into();
    desktop.appearance.decorations = "ssd".into();
    desktop.appearance.menu_side = "right".into();
    desktop.appearance.font_size = Some(16.0);
    desktop.appearance.font_weight = Some(300);
    desktop.appearance.icon_stroke = Some(1.75);
    desktop.ui.density = 0.85;
    desktop.ui.text_scale = 1.25;
    desktop.ui.reduced_motion = true;
    desktop.apps.insert(
        "term".into(),
        AppOverride {
            scheme: Some("ocean".into()),
            mode: Some("light".into()),
            contrast: Some("normal".into()),
            text_scale: Some(1.5),
        },
    );
    let v2 = values(&[
        ("appearance.scheme", json!("forest")),
        ("appearance.style", json!("classic")),
        ("appearance.contrast", json!("high")),
        ("window.style", json!("win11")),
        ("window.decorations", json!("ssd")),
        ("window.menu_side", json!("right")),
        ("appearance.font_size", json!(16.0)),
        ("appearance.font_weight", json!(300)),
        ("appearance.icon_stroke", json!(1.75)),
        ("appearance.density", json!("compact")),
        ("accessibility.text_scale", json!(1.25)),
        ("accessibility.reduced_motion", json!(true)),
        ("apps.term.scheme", json!("ocean")),
        ("apps.term.mode", json!("light")),
        ("apps.term.contrast", json!("normal")),
        ("apps.term.text_scale", json!(1.5)),
    ]);
    let (today, today_accents) =
        crate::resolve_with_embedded_and_accents(&desktop, design::EMBEDDED_DEFAULT_SOURCE)
            .unwrap();
    let (next, next_accents) = resolve(&v2, design::EMBEDDED_DEFAULT_SOURCE).unwrap();
    assert_eq!(today_accents, next_accents);
    assert_eq!(
        today.keys().collect::<Vec<_>>(),
        next.keys().collect::<Vec<_>>()
    );
    for (context, old) in today {
        let mut old = old;
        if let Some(from) = old.provenance.remove("ui.text_scale") {
            old.provenance.insert("accessibility.text_scale".into(), from);
        }
        assert_eq!(
            serde_json::to_value(&old).unwrap(),
            serde_json::to_value(&next[&context]).unwrap(),
            "{context}"
        );
    }
}

/// LOOK SEAM: until S2, the default `mixos` compiles as today's default
/// `studio`, so the default look is today's.
#[test]
fn the_default_scheme_resolves_through_the_look_seam() {
    let next = resolved(&Values::new());
    let today = crate::resolve(&Desktop::default()).unwrap();
    assert_eq!(next["desktop"].scheme, "mixos");
    assert_eq!(
        serde_json::to_value(&next["desktop"].design).unwrap(),
        serde_json::to_value(&today["desktop"].design).unwrap()
    );
}

#[test]
fn density_steps_give_today_s_multipliers() {
    for (step, multiplier) in [("compact", 0.85), ("comfortable", 1.0), ("spacious", 1.2)] {
        let effective = resolved(&values(&[("appearance.density", json!(step))]));
        assert_eq!(effective["desktop"].ui.density, multiplier, "{step}");
    }
}

#[test]
fn non_look_keys_leave_the_effective_map_alone() {
    let base = crate::digest(&resolved(&Values::new())).unwrap();
    let other = values(&[
        ("window.gaps", json!(8)),
        ("input.mouse.speed", json!(70)),
        ("time.applet.show_seconds", json!(true)),
        ("keymaps.dopus.nav.parent", json!(["Ctrl+U"])),
        ("shell.panels.bottom.autohide", json!(true)),
    ]);
    assert_eq!(crate::digest(&resolved(&other)).unwrap(), base);
}
