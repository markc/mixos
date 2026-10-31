// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use crate::registry::tests::{concrete, sample_batch};
use crate::registry::{KeyDef, keys};

/// Each view, decoded with a recording reader, by name: what it read and a
/// debug rendering of what it decoded.
fn each_view(values: &Values) -> Vec<(&'static str, BTreeSet<&'static str>, String)> {
    fn one<T: View + std::fmt::Debug>(
        name: &'static str,
        values: &Values,
    ) -> (&'static str, BTreeSet<&'static str>, String) {
        let r = Reader::recording(values);
        let view = T::read(&r);
        (name, r.seen().into_iter().collect(), format!("{view:?}"))
    }
    vec![
        one::<Appearance>("Appearance", values),
        one::<State>("State", values),
        one::<Window>("Window", values),
        one::<Panels>("Panels", values),
        one::<Wallpaper>("Wallpaper", values),
        one::<Workspaces>("Workspaces", values),
        one::<Displays>("Displays", values),
        one::<Input>("Input", values),
        one::<Keymaps>("Keymaps", values),
        one::<Sound>("Sound", values),
        one::<Power>("Power", values),
        one::<Time>("Time", values),
        one::<Region>("Region", values),
        one::<Accessibility>("Accessibility", values),
        one::<Applications>("Applications", values),
        one::<Xwayland>("Xwayland", values),
        one::<AppLooks>("AppLooks", values),
        one::<Tune>("Tune", values),
        one::<AppDomains>("AppDomains", values),
    ]
}

/// The values of `def` set to a valid non-default sample, with any
/// companion its constraints need.
fn sampled(def: &KeyDef) -> Values {
    let changes = sample_batch(def).into_iter().collect();
    registry::apply(&Values::new(), &changes, &[])
        .unwrap_or_else(|e| panic!("{}: {e:?}", def.path))
}

/// §1.6: every registry key is read by exactly one view, unless it is
/// `export_only` (the Mix exporters read it over the Bus) or live (never
/// stored, so no view may read it). Read means more than looked up: setting
/// the key to a non-default value changes what its view decodes, and no
/// other view's.
#[test]
fn every_key_is_read_by_exactly_one_view() {
    let defaults = each_view(&Values::new());
    for def in keys() {
        let path = concrete(def.path);
        if def.is_live() {
            let readers: Vec<&str> = defaults
                .iter()
                .filter(|(_, seen, _)| seen.contains(def.path))
                .map(|(name, _, _)| *name)
                .collect();
            assert!(readers.is_empty(), "{path}: live, read by {readers:?}");
            continue;
        }
        let values = sampled(def);
        let views = each_view(&values);
        let readers: Vec<&str> = views
            .iter()
            .filter(|(_, seen, _)| seen.contains(def.path))
            .map(|(name, _, _)| *name)
            .collect();
        if def.export_only {
            assert!(readers.is_empty(), "{path}: export only, read by {readers:?}");
            continue;
        }
        assert_eq!(readers.len(), 1, "{path}: read by {readers:?}");
        // The companions of a constrained sample are other keys; only the
        // reader of this one, and of those, may decode differently.
        let companions: BTreeSet<&str> = values
            .keys()
            .filter_map(|k| registry::lookup(k).ok().map(|m| m.def.path))
            .collect();
        for ((name, seen, decoded), (_, _, default)) in views.iter().zip(&defaults) {
            if *name == readers[0] {
                assert_ne!(decoded, default, "{path}: {name} ignores the value");
            } else if seen.is_disjoint(&companions) {
                assert_eq!(decoded, default, "{path}: {name} changed too");
            }
        }
    }
}

#[test]
fn defaults_decode_as_the_registry_and_today_s_model_have_them() {
    let v = Views::of(&Values::new());
    assert_eq!(
        (v.appearance.mode.as_str(), v.appearance.scheme.as_str()),
        ("dark", "mixos")
    );
    assert_eq!(v.appearance.accent, Accent::Auto);
    assert_eq!(v.appearance.density, Density::Comfortable);
    assert_eq!(v.appearance.density.legacy_multiplier(), 1.0);
    assert_eq!(v.window.framing.style, "mac");
    assert_eq!(v.window.framing.decorations, "auto");
    assert_eq!((v.window.active_hint, v.window.gaps), (3, 0));
    assert!(!v.panels.top.enabled && v.panels.bottom.enabled);
    assert_eq!((v.panels.top.thickness, v.panels.bottom.thickness), (32, 40));
    assert_eq!(v.panels.bottom.edge, "bottom");
    assert_eq!(v.panels.bottom.applets, Applets::default(), "Quoin's own");
    assert_eq!(v.input.keyboard.layouts, vec![("us".to_owned(), String::new())]);
    assert_eq!(v.input.mouse.speed, 57);
    assert_eq!(v.accessibility.text_scale, 1.0);
    assert_eq!(v.time.zone, None, "the owner's until authored");
    assert_eq!(v.wallpaper.slideshow.interval_s, 600);
    assert!(v.displays.0.is_empty());
    assert_eq!(v.displays.get("DP-1"), Display::default());
    assert_eq!(v.app_domains, AppDomains::default());
    assert_eq!(v.state, State::default());
}

#[test]
fn families_decode_by_member() {
    let changes = [
        ("displays.DP-2.scale", serde_json::json!(150)),
        ("displays.HDMI-A-1.enabled", false.into()),
        ("wallpaper.same_on_all", false.into()),
        ("wallpaper.image", "/srv/walls/a.png".into()),
        ("wallpaper.outputs.DP-2.image", "/srv/walls/b.png".into()),
        ("apps.term.mode", "light".into()),
        ("keymaps.dopus.nav.parent", serde_json::json!(["Ctrl+U"])),
        (
            "keymaps.comp.spawn.browser",
            serde_json::json!({"command": ["firefox"], "bindings": ["Super+B"]}),
        ),
        ("terminal.font.size", serde_json::json!(15.0)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let values = registry::apply(&Values::new(), &changes, &[]).unwrap();
    let v = Views::of(&values);
    assert_eq!(v.displays.get("DP-2").scale, Some(150));
    assert!(!v.displays.get("HDMI-A-1").enabled);
    assert_eq!(v.wallpaper.image_for("DP-2"), Some("/srv/walls/b.png"));
    assert_eq!(v.wallpaper.image_for("eDP-1"), Some("/srv/walls/a.png"));
    assert_eq!(v.app_looks.0["term"].mode.as_deref(), Some("light"));
    assert_eq!(v.keymaps.app("dopus")["nav.parent"], ["Ctrl+U"]);
    assert!(!v.keymaps.apps.contains_key("comp"), "spawn entries are apart");
    assert_eq!(v.keymaps.spawn["browser"].command, ["firefox"]);
    assert_eq!(v.app_domains.terminal.font.size, 15.0);
}
