// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use serde_json::json;

fn set(changes: &[(&str, Value)]) -> Result<Values, Vec<Diagnostic>> {
    let changes = changes
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    apply(&Values::new(), &changes, &[])
}

fn codes(result: Result<Values, Vec<Diagnostic>>) -> Vec<(String, String)> {
    result
        .expect_err("refused")
        .into_iter()
        .map(|d| (d.code, d.path))
        .collect()
}

/// A pattern made concrete with one sample value per placeholder.
pub(crate) fn concrete(pattern: &str) -> String {
    let token = design::tunables()
        .iter()
        .find_map(|t| t.name.strip_prefix("style."))
        .unwrap();
    let role = design::tunables()
        .iter()
        .find_map(|t| t.name.strip_prefix("chrome."))
        .unwrap();
    pattern
        .split('.')
        .map(|part| match part {
            "<connector>" => "HDMI-A-1",
            "<panel>" => "top",
            "<tablet>" => "pen",
            "<app>" => "term",
            "<action>" => "nav.back",
            "<name>" => "browser",
            "<category>" => "web",
            "<token>" => token,
            "<role>" => role,
            literal => literal,
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn sample_string(grammar: Grammar) -> Value {
    match grammar {
        Grammar::Plain => "Noto Sans".into(),
        Grammar::DirName => "Adwaita".into(),
        Grammar::Connector => "DP-2".into(),
        Grammar::OutputMode => "1920x1080@60.000".into(),
        Grammar::Position => "-1920,0".into(),
        Grammar::Zone => "Australia/Brisbane".into(),
        Grammar::Locale => "en_AU.UTF-8".into(),
        Grammar::DesktopId => "org.example.Files.desktop".into(),
        Grammar::Applet => "clock".into(),
        Grammar::Source => design::EMBEDDED_DEFAULT_SOURCE.into(),
        Grammar::WallpaperColour => "gradient:#102030,#405060@90".into(),
    }
}

/// A valid value of `kind` that differs from `default`.
fn sample(kind: &Kind, default: Option<&Value>) -> Value {
    let differs =
        |candidates: Vec<Value>| candidates.into_iter().find(|c| Some(c) != default).unwrap();
    match *kind {
        Kind::Bool => (default != Some(&json!(true))).into(),
        Kind::Int { min, step, .. } => differs(vec![min.into(), (min + step).into()]),
        Kind::OneOf(set) => differs(set.iter().map(|n| json!(n)).collect()),
        Kind::Number { min, max, .. } => differs(vec![min.into(), max.into()]),
        Kind::Enum(names) | Kind::Known(names) => differs(names.iter().map(|n| json!(n)).collect()),
        Kind::Colour(_) | Kind::ColourOr(..) => "#123456".into(),
        Kind::ConnectorOr(_) => "DP-2".into(),
        Kind::Str(grammar, _) => sample_string(grammar),
        Kind::Path { directory, .. } => if directory {
            "/home/user/Pictures"
        } else {
            "/home/user/Pictures/sea.png"
        }
        .into(),
        Kind::List(of, min, _) => {
            if *of == Kind::XkbLayout {
                return json!([["de", "nodeadkeys"]]);
            }
            Value::Array(vec![sample(of, None); min.max(1)])
        }
        Kind::XkbLayout => json!(["de", ""]),
        Kind::Chords => json!(["Ctrl+Alt+U"]),
        Kind::Spawn => json!({"command": ["term"], "bindings": ["Super+T"]}),
        Kind::Tunable | Kind::AppDomain | Kind::Catalogue(_) => unreachable!(),
    }
}

pub(crate) fn tunable_sample(path: &str) -> Value {
    let t = tunable(path.strip_prefix("tune.").unwrap()).unwrap();
    match (t.kind, &t.range) {
        ("flag", _) => true.into(),
        ("colour", _) => "#123456".into(),
        ("choice", Some(design::TunableRange::Choice { variants })) => variants[0].clone().into(),
        (_, Some(design::TunableRange::Number { min, .. })) => json!(min),
        other => panic!("{path}: {other:?}"),
    }
}

/// A leaf of an app domain, from its module's describe entry.
fn domain_sample(path: &str) -> Vec<(String, Value)> {
    let account = json!([{"id": "acct-1", "name": "Work",
        "url": "https://mail.example.com", "email": "me@example.com"}]);
    let one = |v: Value| vec![(path.to_owned(), v)];
    match path {
        "email.accounts" => return one(account),
        "email.default_account" => {
            return vec![
                ("email.accounts".into(), account),
                (path.into(), "acct-1".into()),
            ];
        }
        "terminal.font.family" => return one("Noto Sans Mono".into()),
        "terminal.env.term" => return one("foot".into()),
        _ => {}
    }
    let schema = app_domains::schema(path).unwrap();
    let default = schema.get("default");
    let differs = |candidates: &[Value]| {
        candidates
            .iter()
            .rev()
            .find(|c| Some(*c) != default)
            .unwrap()
            .clone()
    };
    let names = schema.get("enum").or_else(|| schema.get("names"));
    let value = if let Some(Value::Array(names)) = names {
        differs(names)
    } else if schema["type"] == "boolean" {
        (default != Some(&json!(true))).into()
    } else if let (Some(min), Some(max)) = (schema.get("minimum"), schema.get("maximum")) {
        differs(&[max.clone(), min.clone()])
    } else {
        panic!("{path}: no sample for {schema}")
    };
    one(value)
}

/// The changes that set `def` to a non-default value, with any companion
/// the constraints need.
pub(crate) fn sample_batch(def: &KeyDef) -> Vec<(String, Value)> {
    let path = concrete(def.path);
    let mut batch = match def.kind {
        Kind::AppDomain => return domain_sample(&path),
        Kind::Tunable => vec![(path.clone(), tunable_sample(&path))],
        _ => {
            let m = lookup(&path).unwrap();
            vec![(path.clone(), sample(&def.kind, m.default_value().as_ref()))]
        }
    };
    match path.as_str() {
        "wallpaper.slideshow.enabled" => {
            batch.push(("wallpaper.slideshow.folder".into(), "/srv/walls".into()))
        }
        "displays.HDMI-A-1.enabled" => batch.push(("displays.DP-2.vrr".into(), "off".into())),
        _ => {}
    }
    batch
}

#[test]
fn every_stored_registry_path_is_settable_and_resettable() {
    for def in keys().iter().filter(|d| !d.is_live()) {
        let path = concrete(def.path);
        let batch = sample_batch(def);
        let changes: BTreeMap<String, Value> = batch.iter().cloned().collect();
        let written = apply(&Values::new(), &changes, &[])
            .unwrap_or_else(|e| panic!("{path} = {:?}: {e:?}", changes[&path]));
        assert!(written.contains_key(&path), "{path}: stored");
        validate(&written).unwrap();
        let companions: Values = written
            .iter()
            .filter(|(k, _)| **k != path)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let reset = apply(&written, &BTreeMap::new(), std::slice::from_ref(&path))
            .unwrap_or_else(|e| panic!("{path}: reset {e:?}"));
        assert_eq!(reset, companions, "{path}: reset");
        let nulled = apply(
            &written,
            &BTreeMap::from([(path.clone(), Value::Null)]),
            &[],
        )
        .unwrap_or_else(|e| panic!("{path}: null {e:?}"));
        assert_eq!(nulled, companions, "{path}: null resets");
    }
}

#[test]
fn a_value_equal_to_its_default_is_not_stored() {
    for def in keys().iter().filter(|d| !d.is_live()) {
        let path = concrete(def.path);
        let m = lookup(&path).unwrap();
        if let Some(default) = m.default_value().filter(|d| !d.is_null()) {
            assert_eq!(
                set(&[(path.as_str(), default)]).unwrap(),
                Values::new(),
                "{path}"
            );
        }
    }
    let defaults = Values::new();
    assert_eq!(
        effective(&defaults, "shell.panels.top.thickness"),
        Some(json!(32))
    );
    assert_eq!(
        effective(&defaults, "shell.panels.bottom.thickness"),
        Some(json!(40))
    );
    assert_eq!(
        effective(&defaults, "appearance.scheme"),
        Some(json!("mixos"))
    );
    assert_eq!(effective(&defaults, "time.zone"), None, "owner-supplied");
}

#[test]
fn live_keys_are_refused_by_set_and_reset() {
    let live: Vec<_> = keys().iter().filter(|d| d.is_live()).collect();
    assert_eq!(live.len(), 15);
    for def in live {
        assert!(def.owner.is_some() && def.page.is_some(), "{}", def.path);
        assert_eq!(def.default, DefaultValue::None);
        for value in [json!(true), json!(0.5), json!("x"), Value::Null] {
            let refused = codes(set(&[(def.path, value)]));
            assert_eq!(refused, [("live_key".into(), def.path.into())]);
        }
        let reset = apply(&Values::new(), &BTreeMap::new(), &[def.path.into()]);
        assert_eq!(codes(reset)[0].0, "live_key");
        let stored = Values::from([(def.path.to_owned(), json!(true))]);
        assert_eq!(validate(&stored).unwrap_err()[0].code, "live_key");
    }
    // The refusal is structured: the owner and its commands, null where the
    // owner has no such command yet.
    let message = &set(&[("system.hostname", json!("x"))]).unwrap_err()[0].message;
    let detail: Value = serde_json::from_str(message).unwrap();
    assert_eq!(
        detail,
        json!({"owner": "hostd", "read": "host.info", "write": null})
    );
    let message = &set(&[("power.battery", json!(1))]).unwrap_err()[0].message;
    let detail: Value = serde_json::from_str(message).unwrap();
    assert_eq!(
        detail,
        json!({"owner": "hostd", "read": null, "write": null})
    );
    let described = describe_key(lookup("power.battery").unwrap().def);
    assert_eq!(
        (&described["read"], &described["write"]),
        (&Value::Null, &Value::Null)
    );
    // A prefix of live keys only is refused as live; a mixed one resets the
    // stored keys under it.
    for prefix in ["system.*", "network.*", "bluetooth.*"] {
        let reset = apply(&Values::new(), &BTreeMap::new(), &[prefix.into()]);
        assert_eq!(codes(reset)[0].0, "live_key", "{prefix}");
    }
    let amplified = set(&[("sound.input.amplify", json!(true))]).unwrap();
    let reset = apply(&amplified, &BTreeMap::new(), &["sound.*".into()]).unwrap();
    assert!(reset.is_empty());
}

/// §2.18 as corrected: `apps_default.<category>` is one pattern, so its row
/// is 6 and the Desktop total 165 (150 stored, applied or state; 15 live).
#[test]
fn the_counts_match_the_design() {
    let group = |path: &'static str| -> &'static str {
        match path.split('.').next().unwrap() {
            "apps_default" | "apps_startup" | "xwayland" => "apps_default+",
            "time" | "region" => "time+",
            "network" | "bluetooth" => "network+",
            other => other,
        }
    };
    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for def in keys() {
        let entry = counts.entry(group(def.path)).or_default();
        entry.0 += 1;
        entry.1 += usize::from(def.is_live());
    }
    let expected = BTreeMap::from([
        ("wallpaper", (13, 0)),
        ("appearance", (27, 0)),
        ("window", (22, 0)),
        ("shell", (13, 0)),
        ("workspaces", (4, 0)),
        ("displays", (7, 0)),
        ("sound", (9, 7)),
        ("power", (6, 2)),
        ("input", (23, 0)),
        ("keymaps", (2, 0)),
        ("apps_default+", (6, 0)),
        ("time+", (9, 0)),
        ("system", (4, 4)),
        ("accessibility", (16, 0)),
        ("network+", (2, 2)),
        ("state", (2, 0)),
        ("terminal", (14, 0)),
        ("email", (15, 0)),
        ("ced", (4, 0)),
        ("apps", (4, 0)),
        ("tune", (2, 0)),
    ]);
    assert_eq!(counts, expected);
    let desktop: Vec<_> = keys()
        .iter()
        .filter(|d| {
            !["terminal.", "email.", "ced.", "apps.", "tune."]
                .iter()
                .any(|p| d.path.starts_with(p))
        })
        .collect();
    assert_eq!(desktop.len(), 165);
    assert_eq!(desktop.iter().filter(|d| d.is_live()).count(), 15);
    assert_eq!(desktop.iter().filter(|d| !d.is_live()).count(), 150);
    assert_eq!(keys().len(), 204);
}

#[test]
fn every_path_is_unique_and_every_entry_is_well_formed() {
    let mut paths = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for def in keys() {
        assert!(paths.insert(def.path), "duplicate {}", def.path);
        for part in def.path.split('.').filter(|p| p.starts_with('<')) {
            assert!(
                PLACEHOLDERS.iter().any(|(name, _)| *name == part),
                "{}: {part}",
                def.path
            );
        }
        match def.class {
            Class::State => assert!(def.page.is_none() && def.owner.is_some()),
            Class::Applied | Class::Live => assert!(def.owner.is_some(), "{}", def.path),
            Class::Stored => assert!(def.page.is_some(), "{}", def.path),
        }
        if def.page.is_some() {
            assert!(labels.insert(label_id(def.path)), "label {}", def.path);
        }
        for id in def.constraints {
            assert!(
                constraint::CONSTRAINTS.iter().any(|c| c.id == *id),
                "{}: {id}",
                def.path
            );
        }
        // Every concrete key looks itself up.
        let path = concrete(def.path);
        assert_eq!(lookup(&path).unwrap().def.path, def.path, "{path}");
    }
}

#[test]
fn names_and_defaults_agree_with_today_s_model_and_design() {
    // The registry lists the derived scheme first, then the authored ones.
    assert_eq!(appearance::SCHEMES[0], design::Scheme::Mixos.name());
    let schemes: Vec<&str> = design::Scheme::AUTHORED.iter().map(|s| s.name()).collect();
    assert_eq!(&appearance::SCHEMES[1..], schemes.as_slice());
    let styles: Vec<&str> = design::Style::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(appearance::STYLES, styles.as_slice());
    let today = crate::model::Appearance::default();
    for (path, value) in [
        ("window.decorations", &today.decorations),
        ("window.caption_side", &today.caption_side),
        ("window.style", &today.window_style),
        ("window.menu_style", &today.menu_style),
        ("window.menu_side", &today.menu_side),
        ("window.actions_place", &today.actions_place),
        ("window.actions_align", &today.actions_align),
        ("window.title_align", &today.title_align),
        ("appearance.mode", &today.mode),
        ("appearance.contrast", &today.contrast),
    ] {
        assert_eq!(
            effective(&Values::new(), path),
            Some(json!(value)),
            "{path}"
        );
    }
    assert_eq!(
        effective(&Values::new(), "shell.panels.bottom.thickness"),
        Some(json!(crate::model::Panel::default().thickness))
    );
}

#[test]
fn domain_leaves_are_exactly_the_modules_fields() {
    let registered: BTreeSet<&str> = keys()
        .iter()
        .filter(|d| d.kind == Kind::AppDomain)
        .map(|d| d.path)
        .collect();
    let schemas = app_domains::schemas();
    let described: BTreeSet<&str> = schemas
        .keys()
        .map(String::as_str)
        .filter(|p| p.contains('.'))
        .collect();
    assert_eq!(registered, described);
}

#[test]
fn domain_values_are_checked_by_their_modules() {
    // The module's own range and cross-field rules.
    assert_eq!(
        codes(set(&[("terminal.padding", json!(3))])),
        [("validation_failed".into(), "terminal.padding".into())]
    );
    assert_eq!(
        codes(set(&[("email.default_account", json!("nobody"))]))[0].1,
        "email.default_account"
    );
    assert_eq!(
        codes(set(&[("ced.editor.font.family", json!("Inter"))]))[0].1,
        "ced.editor.font.family"
    );
    let ok = set(&[("terminal.font.size", json!(15))]).unwrap();
    assert_eq!(ok["terminal.font.size"], json!(15));
}

#[test]
fn values_are_normalised() {
    let values = set(&[
        ("appearance.accent", json!("#A1B2C3")),
        ("window.gaps", json!(8.0)),
        ("wallpaper.color", json!("gradient:#AABBCC,#DDEEFF@45")),
        ("appearance.frosted.opacity", json!(0.35)),
        ("keymaps.comp.spawn.term", json!({"command": ["term"]})),
    ])
    .unwrap();
    assert_eq!(values["appearance.accent"], json!("#a1b2c3"));
    assert_eq!(values["window.gaps"], json!(8));
    assert_eq!(
        values["wallpaper.color"],
        json!("gradient:#aabbcc,#ddeeff@45")
    );
    assert_eq!(
        values["keymaps.comp.spawn.term"],
        json!({"command": ["term"], "bindings": []})
    );
    validate(&values).unwrap();
    // A stored form that is not canonical is refused.
    let raw = Values::from([("window.gaps".into(), json!("8"))]);
    assert!(validate(&raw).is_err());
    let upper = Values::from([("appearance.accent".into(), json!("#A1B2C3"))]);
    assert!(validate(&upper).is_err());
    let default = Values::from([("window.gaps".into(), json!(0))]);
    assert!(validate(&default).is_err());
}

#[test]
fn kinds_refuse_what_they_do_not_accept() {
    for (path, value) in [
        ("appearance.mode", json!("dusk")),
        ("appearance.accent", json!("teal")),
        ("appearance.accent", json!("#12345678")),
        ("appearance.window_bg", json!("#1234")),
        ("appearance.frosted.opacity", json!(0.33)),
        ("appearance.frosted.thickness", json!(14)),
        ("appearance.font_weight", json!(450)),
        ("appearance.icon_stroke", json!(1.1)),
        ("appearance.accent_palette", json!(["#000000"])),
        ("window.gaps", json!(8.5)),
        ("window.style", json!("Bad Name")),
        ("wallpaper.image", json!("relative.png")),
        ("wallpaper.image", json!("/home/user/a.gif")),
        ("wallpaper.slideshow.interval_s", json!(700)),
        ("wallpaper.color", json!("gradient:#000000,#ffffff@400")),
        ("shell.panels.bottom.thickness", json!(48)),
        ("shell.panels.bottom.mode", json!("hidden")),
        ("displays.HDMI-A-1.mode", json!("1920x1080")),
        ("displays.HDMI-A-1.scale", json!(103)),
        ("input.keyboard.layouts", json!([])),
        ("input.keyboard.repeat_rate", json!(7)),
        ("apps_default.web", json!("firefox")),
        ("time.zone", json!("../etc/passwd")),
        ("keymaps.dopus.nav.back", json!(["Hyper+X"])),
        ("keymaps.dopus.nav.back", json!(["F1", "F1"])),
        ("keymaps.comp.spawn.term", json!({"command": []})),
        (
            "keymaps.comp.spawn.term",
            json!({"command": ["x"], "env": {}}),
        ),
        ("apps.term.mode", json!("auto")),
        ("state.appearance.mode_override", json!("auto")),
        ("appearance.source", json!("run_argv([\"do-not-execute\"])")),
        ("appearance.source", json!("")),
    ] {
        assert!(set(&[(path, value.clone())]).is_err(), "{path} = {value}");
    }
}

#[test]
fn placeholders_are_typed_and_unknown_paths_refused() {
    for path in [
        "shell.panels.left.enabled",
        "displays.HDMI A.enabled",
        "apps_default.browser",
        "apps.bad app.mode",
        // A rest capture never excuses the captures before it.
        "keymaps.invalid app!.foo",
        "keymaps.bad app.nav.back",
        "keymaps..nav.back",
        "appearance.nope",
        "ui.density",
        "shell.page_order",
        "appearance.decorations",
    ] {
        assert!(lookup(path).is_err(), "{path}");
    }
    assert_eq!(
        lookup("keymaps.comp.spawn.term").unwrap().def.path,
        "keymaps.comp.spawn.<name>"
    );
    assert_eq!(
        lookup("keymaps.comp.window.close").unwrap().def.path,
        "keymaps.<app>.<action>"
    );
}

#[test]
fn batches_are_bounded_all_or_nothing_and_prefix_resettable() {
    let too_many: BTreeMap<String, Value> = (0..65)
        .map(|n| (format!("keymaps.dopus.a{n}"), json!(["Ctrl+A"])))
        .collect();
    assert!(apply(&Values::new(), &too_many, &[]).is_err());
    let partial = set(&[
        ("appearance.mode", json!("light")),
        ("appearance.typo", json!(1)),
    ]);
    assert!(partial.is_err());
    let values = set(&[
        ("appearance.mode", json!("light")),
        ("appearance.frosted.panel", json!(true)),
        ("window.gaps", json!(8)),
    ])
    .unwrap();
    let both = apply(
        &values,
        &BTreeMap::from([("window.gaps".into(), json!(4))]),
        &["window.gaps".into()],
    );
    assert!(both.is_err(), "the same key set and reset");
    let page = apply(&values, &BTreeMap::new(), &["appearance.*".into()]).unwrap();
    assert_eq!(page, Values::from([("window.gaps".into(), json!(8))]));
    let group = apply(&values, &BTreeMap::new(), &["appearance.frosted.*".into()]).unwrap();
    assert_eq!(group.len(), 2);
    assert!(apply(&values, &BTreeMap::new(), &["nowhere.*".into()]).is_err());
}

#[test]
fn family_limits_hold() {
    let outputs: BTreeMap<String, Value> = (0..17)
        .map(|n| (format!("displays.DP-{n}.vrr"), json!("off")))
        .collect();
    assert!(apply(&Values::new(), &outputs, &[]).is_err());
    let sixteen: BTreeMap<String, Value> = outputs.into_iter().take(16).collect();
    apply(&Values::new(), &sixteen, &[]).unwrap();
}

#[test]
fn c1_c2_c4_record_when_a_key_takes_effect_and_refuse_nothing() {
    for (path, value) in [
        ("appearance.frosted.panel", json!(true)),
        ("window.focus_delay_ms", json!(500)),
        (
            "wallpaper.outputs.HDMI-A-1.image",
            json!("/home/user/a.png"),
        ),
    ] {
        set(&[(path, value)]).unwrap();
        let def = lookup(path).unwrap().def;
        assert!(def.used_when.is_some(), "{path}");
        let described = describe_key(def);
        assert!(described["used_when"].is_string(), "{path}");
    }
    for id in ["C1", "C2", "C4", "C7"] {
        let c = constraint::CONSTRAINTS.iter().find(|c| c.id == id).unwrap();
        assert!(c.check.is_none(), "{id}");
    }
}

#[test]
fn c3_a_slideshow_needs_a_folder() {
    assert_eq!(
        codes(set(&[("wallpaper.slideshow.enabled", json!(true))])),
        [(
            "constraint_failed".into(),
            "wallpaper.slideshow.enabled".into()
        )]
    );
    let on = set(&[
        ("wallpaper.slideshow.enabled", json!(true)),
        ("wallpaper.slideshow.folder", json!("/srv/walls")),
    ])
    .unwrap();
    // Removing the folder alone is refused: the constraint moves with it.
    let refused = apply(
        &on,
        &BTreeMap::new(),
        &["wallpaper.slideshow.folder".into()],
    );
    assert_eq!(codes(refused)[0].0, "constraint_failed");
}

#[test]
fn c5_mirrors_never_chain_or_mirror_themselves() {
    assert!(set(&[("displays.DP-1.mirror_of", json!("DP-1"))]).is_err());
    assert!(
        set(&[
            ("displays.DP-1.mirror_of", json!("DP-2")),
            ("displays.DP-2.mirror_of", json!("DP-3")),
        ])
        .is_err()
    );
    set(&[
        ("displays.DP-1.mirror_of", json!("DP-3")),
        ("displays.DP-2.mirror_of", json!("DP-3")),
    ])
    .unwrap();
}

#[test]
fn c6_one_known_output_stays_enabled() {
    assert_eq!(
        codes(set(&[("displays.eDP-1.enabled", json!(false))])),
        [("constraint_failed".into(), "displays.eDP-1.enabled".into())]
    );
    set(&[
        ("displays.eDP-1.enabled", json!(false)),
        ("displays.HDMI-A-1.scale", json!(150)),
    ])
    .unwrap();
    assert!(
        set(&[
            ("displays.eDP-1.enabled", json!(false)),
            ("displays.HDMI-A-1.enabled", json!(false)),
        ])
        .is_err()
    );
}

#[test]
fn c7_a_panel_output_is_all_or_a_connector() {
    set(&[("shell.panels.bottom.output", json!("HDMI-A-1"))]).unwrap();
    assert!(set(&[("shell.panels.bottom.output", json!("left screen"))]).is_err());
}

#[test]
fn c8_a_chord_binds_one_action_per_app() {
    let refused = codes(set(&[
        ("keymaps.dopus.nav.back", json!(["Alt+ArrowLeft"])),
        ("keymaps.dopus.nav.parent", json!(["Alt+ArrowLeft"])),
    ]));
    assert_eq!(refused[0].0, "constraint_failed");
    // Another app may use the same chord.
    set(&[
        ("keymaps.dopus.nav.back", json!(["Alt+ArrowLeft"])),
        ("keymaps.ced.nav.back", json!(["Alt+ArrowLeft"])),
    ])
    .unwrap();
    // Spawn bindings are the compositor's too.
    assert!(
        set(&[
            ("keymaps.comp.window.close", json!(["Super+Q"])),
            (
                "keymaps.comp.spawn.term",
                json!({"command": ["term"], "bindings": ["Super+Q"]})
            ),
        ])
        .is_err()
    );
    // Within one list, as today.
    assert!(set(&[("keymaps.dopus.nav.back", json!(["F2", "F2"]))]).is_err());
}

#[test]
fn c8_is_enforced_on_write_and_a_stored_conflict_still_loads() {
    let stored = Values::from([
        ("keymaps.dopus.nav.back".into(), json!(["Alt+ArrowLeft"])),
        ("keymaps.dopus.nav.parent".into(), json!(["Alt+ArrowLeft"])),
    ]);
    validate(&stored).expect("a stored conflict never bricks the profile");
    let found = constraint::conflicts(&stored);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "constraint_failed");
    assert_eq!(found[0].path, "keymaps.dopus.nav.parent");
    // An unrelated write goes through, conflict and all.
    let next = apply(
        &stored,
        &BTreeMap::from([("window.gaps".into(), json!(8))]),
        &[],
    )
    .unwrap();
    assert_eq!(constraint::conflicts(&next), found);
    // A write that adds a conflict is refused.
    let worse = apply(
        &stored,
        &BTreeMap::from([("keymaps.dopus.nav.home".into(), json!(["Alt+ArrowLeft"]))]),
        &[],
    );
    assert_eq!(codes(worse)[0].0, "constraint_failed");
    // Resolving it is a write like any other.
    let fixed = apply(
        &stored,
        &BTreeMap::new(),
        &["keymaps.dopus.nav.parent".into()],
    )
    .unwrap();
    assert!(constraint::conflicts(&fixed).is_empty());
}

#[test]
fn panels_may_not_share_an_edge() {
    assert_eq!(
        codes(set(&[
            ("shell.panels.top.enabled", json!(true)),
            ("shell.panels.top.edge", json!("bottom")),
        ]))[0]
            .0,
        "constraint_failed"
    );
    // A disabled panel may sit anywhere.
    set(&[("shell.panels.top.edge", json!("bottom"))]).unwrap();
}

#[test]
fn the_compositor_never_binds_a_bare_function_key() {
    for chord in ["F1", "F12", "f1", "f12"] {
        assert_eq!(
            codes(set(&[("keymaps.comp.window.close", json!([chord]))]))[0].0,
            "constraint_failed"
        );
        assert!(
            set(&[(
                "keymaps.comp.spawn.term",
                json!({"command": ["term"], "bindings": [chord]})
            )])
            .is_err()
        );
    }
    set(&[("keymaps.comp.window.close", json!(["Super+F4", "F13"]))]).unwrap();
    // An app's own keymap is the person's: a bare F-key is theirs to bind.
    set(&[("keymaps.dopus.nav.back", json!(["F2"]))]).unwrap();
    assert!(!constraint::bare_function_key("F01"));
    assert!(!constraint::bare_function_key("Ctrl+F1"));
}

#[test]
fn tunables_are_the_design_catalogue_with_r4_codes() {
    for t in design::tunables() {
        let path = format!("tune.{}", t.name);
        assert!(lookup(&path).is_ok(), "{path}");
    }
    assert_eq!(
        codes(set(&[("tune.style.no_such_token", json!(1))]))[0].0,
        "unknown_tunable"
    );
    let first = |kind: &str| {
        let t = design::tunables().iter().find(|t| t.kind == kind).unwrap();
        format!("tune.{}", t.name)
    };
    let flag = first("flag");
    assert_eq!(codes(set(&[(flag.as_str(), json!(3))]))[0].0, "wrong_kind");
    let number = first("number");
    let Some(design::TunableRange::Number { max, .. }) =
        tunable(number.strip_prefix("tune.").unwrap())
            .unwrap()
            .range
    else {
        unreachable!()
    };
    assert_eq!(
        codes(set(&[(number.as_str(), json!(max + 1.0))]))[0].0,
        "out_of_range"
    );
    let choice = first("choice");
    assert_eq!(
        codes(set(&[(choice.as_str(), json!("no-such"))]))[0].0,
        "unknown_choice"
    );
    let colour = first("colour");
    assert_eq!(
        codes(set(&[(colour.as_str(), json!("red"))]))[0].0,
        "wrong_kind"
    );
    // At most MAX_TUNE entries, over two batches (a batch is at most 64).
    let names: Vec<String> = design::tunables()
        .iter()
        .take(MAX_TUNE + 1)
        .map(|t| format!("tune.{}", t.name))
        .collect();
    assert!(names.len() > MAX_TUNE, "the catalogue outgrows the bound");
    let mut values = Values::new();
    for chunk in names[..MAX_TUNE].chunks(MAX_BATCH) {
        let changes = chunk
            .iter()
            .map(|n| (n.clone(), tunable_sample(n)))
            .collect();
        values = apply(&values, &changes, &[]).unwrap();
    }
    let one_more = BTreeMap::from([(names[MAX_TUNE].clone(), tunable_sample(&names[MAX_TUNE]))]);
    assert!(apply(&values, &one_more, &[]).is_err());
}

#[test]
fn describe_lists_every_key_and_filters_by_prefix() {
    let all = describe(None);
    assert_eq!(all["keys"].as_array().unwrap().len(), keys().len());
    assert!(all["tunables"].is_array());
    let page = describe(Some("appearance"));
    let paths: Vec<&str> = page["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 27);
    assert!(paths.iter().all(|p| p.starts_with("appearance.")));
    assert!(page.get("tunables").is_none());
    let hostname = describe_key(lookup("system.hostname").unwrap().def);
    assert_eq!(hostname["class"], "live");
    assert!(hostname.get("default").is_none());
    let thickness = describe_key(lookup("shell.panels.top.thickness").unwrap().def);
    assert_eq!(thickness["default_by"], json!({"top": 32, "bottom": 40}));
}

#[test]
fn accent_swatches_are_described_with_both_modes() {
    use design::cosmic_tables::{DARK_PALETTE, LIGHT_PALETTE, SWATCH_NAMES};
    assert_eq!(appearance::ACCENT_WORDS[0], "auto");
    assert_eq!(&appearance::ACCENT_WORDS[1..], SWATCH_NAMES.as_slice());
    let accent = describe_key(lookup("appearance.accent").unwrap().def);
    let swatches = accent["swatches"].as_array().unwrap();
    assert_eq!(swatches.len(), 9);
    for (swatch, name) in swatches.iter().zip(SWATCH_NAMES) {
        assert_eq!(swatch["name"], name);
        for (mode, palette) in [("dark", &DARK_PALETTE), ("light", &LIGHT_PALETTE)] {
            let hex = palette.swatch(name).unwrap().to_hex()[..7].to_ascii_lowercase();
            assert_eq!(swatch[mode], json!(hex), "{name} {mode}");
        }
    }
    assert_eq!(swatches[8]["dark"], "#cabab4");
}

#[test]
fn every_int_step_is_positive_and_spawn_entries_are_stored() {
    fn steps(kind: &Kind) -> bool {
        match kind {
            Kind::Int { step, .. } => *step > 0,
            Kind::List(of, ..) => steps(of),
            _ => true,
        }
    }
    for def in keys() {
        assert!(steps(&def.kind), "{}", def.path);
    }
    let spawn = lookup("keymaps.comp.spawn.term").unwrap().def;
    assert_eq!((spawn.class, spawn.owner), (Class::Stored, None));
    // A zero step is never a division: the guard holds even for a bad table.
    let zero = Kind::Int {
        min: 0,
        max: 10,
        step: 0,
    };
    assert_eq!(kind::check(&zero, "x", &json!(7)).unwrap(), json!(7));
}

#[test]
fn the_design_source_is_compiled_by_its_kind() {
    set(&[("appearance.source", json!(design::EMBEDDED_DEFAULT_SOURCE))]).unwrap();
    let broken = codes(set(&[("appearance.source", json!("{\"v1\":"))]));
    assert_eq!(
        broken,
        [("validation_failed".into(), "appearance.source".into())]
    );
}

#[test]
fn app_domains_report_every_error() {
    let errors = set(&[
        ("terminal.padding", json!("wide")),
        ("terminal.cursor.shape", json!(5)),
    ])
    .unwrap_err();
    let paths: Vec<&str> = errors.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["terminal.cursor.shape", "terminal.padding"]);
    // Range checks: one per record, from every record.
    let errors = set(&[
        ("terminal.padding", json!(3)),
        ("ced.editor.font.family", json!("Inter")),
    ])
    .unwrap_err();
    let paths: Vec<&str> = errors.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["terminal.padding", "ced.editor.font.family"]);
}
