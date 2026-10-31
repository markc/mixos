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
    resolve(values, design::EMBEDDED_DEFAULT_SOURCE).unwrap()
}

/// Today's effective map for `desktop`, each context's sidecar accent
/// sealed in as v2 seals it: the one shape change v2 makes.
fn today_sealed(desktop: &Desktop) -> BTreeMap<String, Effective> {
    let (mut today, accents) =
        crate::v1::resolve_with_embedded_and_accents(desktop, design::EMBEDDED_DEFAULT_SOURCE)
            .unwrap();
    for (context, e) in &mut today {
        e.accent = accents.get(context).copied();
    }
    today
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
    assert_eq!(
        effective["app:term"].provenance["appearance.mode"],
        "apps.term"
    );
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

/// The first path at which two JSON values differ, with both sides.
fn first_difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter().find_map(|k| {
                let null = Value::Null;
                let here = format!("{path}.{k}");
                first_difference(&here, a.get(k).unwrap_or(&null), b.get(k).unwrap_or(&null))
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (x, y))| first_difference(&format!("{path}[{i}]"), x, y)),
        _ if a == b => None,
        _ => Some(format!("{path}: today {a} / v2 {b}")),
    }
}

/// One choice of look, as today's record and as v2 values.
struct Profile {
    name: &'static str,
    today: Desktop,
    v2: Vec<(&'static str, Value)>,
    /// The key that decides the desktop mode, from the inputs.
    decided: &'static str,
}

/// A look that touches every v1 look field, an app override per axis, and
/// a key under every non-look v2 prefix (which must move nothing).
fn rich() -> Profile {
    let mut today = Desktop::default();
    today.appearance.scheme = "forest".into();
    today.appearance.style = Some("classic".into());
    today.appearance.contrast = "high".into();
    today.appearance.window_style = "win11".into();
    today.appearance.decorations = "ssd".into();
    today.appearance.caption_side = "left".into();
    today.appearance.menu_style = "button".into();
    today.appearance.menu_side = "right".into();
    today.appearance.actions_place = "titlebar".into();
    today.appearance.actions_align = "left".into();
    today.appearance.title_align = "left".into();
    today.appearance.font_family = Some("Noto Sans".into());
    today.appearance.font_size = Some(16.0);
    today.appearance.font_weight = Some(300);
    today.appearance.mono_family = Some("Hack".into());
    today.appearance.icon_stroke = Some(1.75);
    today.ui.density = 0.85;
    today.ui.text_scale = 1.25;
    today.ui.reduced_motion = true;
    today.shell.panels.get_mut("bottom").unwrap().thickness = 56;
    today.keymaps.insert(
        "dopus".into(),
        [("nav.parent".into(), vec!["Ctrl+U".into()])].into(),
    );
    today.terminal.font.size = 15.0;
    today.ced.editor.padding = 8;
    today.email.mail.page_size = 50;
    today.apps.insert(
        "term".into(),
        AppOverride {
            scheme: Some("ocean".into()),
            contrast: Some("normal".into()),
            text_scale: Some(1.5),
            ..AppOverride::default()
        },
    );
    today.apps.insert(
        "ced".into(),
        AppOverride {
            mode: Some("light".into()),
            ..AppOverride::default()
        },
    );
    let v2 = vec![
        ("appearance.scheme", json!("forest")),
        ("appearance.style", json!("classic")),
        ("appearance.contrast", json!("high")),
        ("window.style", json!("win11")),
        ("window.decorations", json!("ssd")),
        ("window.caption_side", json!("left")),
        ("window.menu_style", json!("button")),
        ("window.menu_side", json!("right")),
        ("window.actions_place", json!("titlebar")),
        ("window.actions_align", json!("left")),
        ("window.title_align", json!("left")),
        ("appearance.font_family", json!("Noto Sans")),
        ("appearance.font_size", json!(16.0)),
        ("appearance.font_weight", json!(300)),
        ("appearance.mono_family", json!("Hack")),
        ("appearance.icon_stroke", json!(1.75)),
        ("appearance.density", json!("compact")),
        ("accessibility.text_scale", json!(1.25)),
        ("accessibility.reduced_motion", json!(true)),
        ("apps.term.scheme", json!("ocean")),
        ("apps.term.contrast", json!("normal")),
        ("apps.term.text_scale", json!(1.5)),
        ("apps.ced.mode", json!("light")),
        // Every non-look prefix: none of these reaches the effective map.
        ("window.gaps", json!(8)),
        ("window.focus_follows_cursor", json!(true)),
        ("shell.panels.bottom.thickness", json!(56)),
        ("shell.panels.bottom.autohide", json!(true)),
        ("wallpaper.image", json!("/srv/walls/a.png")),
        ("workspaces.wraparound", json!(false)),
        ("displays.DP-2.scale", json!(150)),
        ("input.mouse.speed", json!(70)),
        ("input.keyboard.repeat_rate", json!(30)),
        ("keymaps.dopus.nav.parent", json!(["Ctrl+U"])),
        ("sound.output.amplify", json!(false)),
        ("power.profile", json!("performance")),
        ("time.zone", json!("Australia/Brisbane")),
        ("time.applet.show_seconds", json!(true)),
        ("region.formats", json!("en_AU.UTF-8")),
        ("accessibility.invert", json!(true)),
        ("apps_default.web", json!("firefox.desktop")),
        ("apps_startup.entries", json!(["org.example.Clock.desktop"])),
        ("xwayland.scaling", json!("gaming")),
        ("terminal.font.size", json!(15.0)),
        ("ced.editor.padding", json!(8)),
        ("email.mail.page_size", json!(50)),
        ("appearance.icon_theme", json!("Adwaita")),
    ];
    Profile {
        name: "rich",
        today,
        v2,
        decided: "profile",
    }
}

/// `profile` with today's mode `mode` and v2's mode choices `v2_mode`.
fn with_mode(
    profile: &Profile,
    name: &'static str,
    mode: &str,
    v2_mode: &[(&'static str, Value)],
    decided: &'static str,
) -> Profile {
    let mut today = profile.today.clone();
    today.appearance.mode = mode.into();
    let mut v2 = profile.v2.clone();
    v2.extend(v2_mode.iter().cloned());
    Profile {
        name,
        today,
        v2,
        decided,
    }
}

/// Today's resolver and v2 agree on the same choices, field for field, for
/// both modes and for `auto` resolved by each state key, today's sidecar
/// accent sealed in as v2 seals it. Provenance names the v2 keys
/// (`accessibility.text_scale` for `ui.text_scale`) and, under `auto`, the
/// state key that decided the mode; nothing else differs. A failure names
/// the first differing key.
#[test]
fn resolve_v2_reproduces_today_s_look_for_the_same_choices() {
    let plain = Profile {
        name: "default",
        today: Desktop::default(),
        // Today's default scheme; v2's is `mixos` (the look seam).
        v2: vec![("appearance.scheme", json!("studio"))],
        decided: "profile",
    };
    let rich = rich();
    // A custom source that orders the app axis and names one app (an empty
    // block: the compile must claim it): every app compiles alone.
    let source = design::EMBEDDED_DEFAULT_SOURCE
        .replacen(
            "resolution_order: [\"scheme\", \"mode\"]",
            "resolution_order: [\"scheme\", \"mode\", \"app\"]",
            1,
        )
        .replacen(
            "modifiers: [\n",
            "modifiers: [\n      { when: { app: \"workshop\" } },\n",
            1,
        );
    assert_ne!(
        source,
        design::EMBEDDED_DEFAULT_SOURCE,
        "the fixture orders app"
    );
    assert!(
        crate::resolve::claims_app_axis(&crate::resolve::parsed(&source).unwrap()),
        "the fixture claims the app axis"
    );
    let mut custom = self::rich();
    custom.name = "custom source";
    custom.today.appearance.source = Some(source.clone());
    custom.v2.push(("appearance.source", json!(source)));
    let auto = |daylight: Option<bool>, over: Option<&str>| {
        let mut keys = vec![("appearance.mode", json!("auto"))];
        if let Some(d) = daylight {
            keys.push(("state.appearance.daylight", json!(d)));
        }
        if let Some(o) = over {
            keys.push(("state.appearance.mode_override", json!(o)));
        }
        keys
    };
    let mut profiles = Vec::new();
    for base in [&plain, &rich, &custom] {
        // (name, today's mode, v2's mode keys, the key that decides it)
        let modes = [
            ("dark", "dark", vec![], "profile"),
            (
                "light",
                "light",
                vec![("appearance.mode", json!("light"))],
                "profile",
            ),
            (
                "auto, day",
                "light",
                auto(Some(true), None),
                "state.appearance.daylight",
            ),
            (
                "auto, night",
                "dark",
                auto(Some(false), None),
                "state.appearance.daylight",
            ),
            (
                "auto, unknown",
                "dark",
                auto(None, None),
                "auto_without_daylight",
            ),
            (
                "auto, override",
                "dark",
                auto(Some(true), Some("dark")),
                "state.appearance.mode_override",
            ),
        ];
        for (name, mode, keys, decided) in modes {
            profiles.push((base.name, with_mode(base, name, mode, &keys, decided)));
        }
    }
    for (base, profile) in &profiles {
        let case = format!("{base} / {}", profile.name);
        let v2 = values(&profile.v2);
        let today = today_sealed(&profile.today);
        let next = resolved(&v2);
        assert!(
            next.values().all(|e| e.accent.is_some()),
            "{case}: every context seals its accent"
        );
        assert_eq!(
            today.keys().collect::<Vec<_>>(),
            next.keys().collect::<Vec<_>>(),
            "{case}: contexts"
        );
        let decided = profile.decided;
        for (context, mut old) in today {
            if let Some(from) = old.provenance.remove("ui.text_scale") {
                old.provenance
                    .insert("accessibility.text_scale".into(), from);
            }
            if old.provenance["appearance.mode"] == "profile" {
                old.provenance
                    .insert("appearance.mode".into(), decided.into());
            }
            let old = serde_json::to_value(&old).unwrap();
            let new = serde_json::to_value(&next[&context]).unwrap();
            if let Some(difference) = first_difference("", &old, &new) {
                panic!("{case}: {context}{difference}");
            }
        }
    }
}

/// The equivalence clause: for every authored scheme, every
/// `appearance.style` (and the scheme's own), mode and contrast, with the
/// accent `auto`, no tints, `round` and `comfortable` (all defaults), v2's
/// effective map is today's byte for byte, today's accent sealed in. Every
/// light case also carries an `apps.term.scheme` override (its own context,
/// its own compile and sealed accent), and every high-contrast case pins
/// the source explicitly (`appearance.source`), so both run across every
/// scheme and style without multiplying the matrix.
#[test]
fn every_authored_scheme_at_the_defaults_is_today_s_look() {
    let styles = std::iter::once(None).chain(design::Style::ALL.map(Some));
    for style in styles {
        for scheme in design::Scheme::AUTHORED {
            for mode in ["dark", "light"] {
                for contrast in ["normal", "high"] {
                    let mut today = Desktop::default();
                    let mut v2 = vec![
                        ("appearance.scheme", json!(scheme.name())),
                        ("appearance.mode", json!(mode)),
                        ("appearance.contrast", json!(contrast)),
                    ];
                    today.appearance.scheme = scheme.name().into();
                    today.appearance.mode = mode.into();
                    today.appearance.contrast = contrast.into();
                    if let Some(style) = style {
                        today.appearance.style = Some(style.name().into());
                        v2.push(("appearance.style", json!(style.name())));
                    }
                    if mode == "light" {
                        let other = if scheme == design::Scheme::Ocean {
                            "forest"
                        } else {
                            "ocean"
                        };
                        today.apps.insert(
                            "term".into(),
                            AppOverride {
                                scheme: Some(other.into()),
                                ..AppOverride::default()
                            },
                        );
                        v2.push(("apps.term.scheme", json!(other)));
                    }
                    if contrast == "high" {
                        today.appearance.source = Some(design::EMBEDDED_DEFAULT_SOURCE.into());
                        v2.push(("appearance.source", json!(design::EMBEDDED_DEFAULT_SOURCE)));
                    }
                    let next = resolved(&values(&v2));
                    let mut today = today_sealed(&today);
                    for e in today.values_mut() {
                        let from = e.provenance.remove("ui.text_scale").unwrap();
                        e.provenance.insert("accessibility.text_scale".into(), from);
                    }
                    assert_eq!(
                        serde_json::to_string(&today).unwrap(),
                        serde_json::to_string(&next).unwrap(),
                        "{scheme:?} {style:?} {mode} {contrast}"
                    );
                }
            }
        }
    }
}

/// The two background tints keep their alpha (`ColourOr(.., Alpha::Any)`,
/// §2.2): `#rrggbbaa` resolves, and the alpha reaches the compile.
#[test]
fn background_tints_keep_their_alpha() {
    let digest = |bg: &str| {
        crate::digest(&resolved(&values(&[
            ("appearance.window_bg", json!(bg)),
            ("appearance.container_bg", json!(bg)),
        ])))
        .unwrap()
    };
    assert_ne!(digest("#20242a80"), digest("#20242aff"));
    assert_ne!(
        digest("#20242a80"),
        crate::digest(&resolved(&Values::new())).unwrap()
    );
}

/// A tunable's number is stored in one form, so `5` and `5.0` seal the
/// same look: `registry::apply` keeps the kind check's canonical value
/// (whole for a radius, a float for a number).
#[test]
fn a_tunable_s_number_seals_in_one_form() {
    let number = design::tunables()
        .iter()
        .find(|t| t.kind == "number" && t.name.starts_with("style."))
        .expect("a number tunable");
    let path = format!("tune.{}", number.name);
    let Some(design::TunableRange::Number { min, .. }) = &number.range else {
        panic!("{path}: a number range");
    };
    let whole = min.ceil();
    for (path, a, b) in [
        ("tune.style.radius", json!(5), json!(5.0)),
        (path.as_str(), json!(whole as i64), json!(whole)),
    ] {
        let a = resolved(&values(&[(path, a)]));
        let b = resolved(&values(&[(path, b)]));
        assert_eq!(
            crate::digest(&a).unwrap(),
            crate::digest(&b).unwrap(),
            "{path}"
        );
    }
}

/// The default scheme is the derived `mixos`, compiled as `design` compiles
/// it, and it is not today's `studio`.
#[test]
fn the_default_scheme_is_the_derived_mixos() {
    let next = resolved(&Values::new());
    let desktop = &next["desktop"];
    assert_eq!(desktop.scheme, "mixos");
    let doc = crate::resolve::parsed(design::EMBEDDED_DEFAULT_SOURCE).unwrap();
    let mixos = match design::compile_design(
        &doc,
        design::DesignContext {
            scheme: design::Scheme::Mixos,
            ..Default::default()
        },
    ) {
        design::DesignCompileResult::Success(success) => success.candidate,
        design::DesignCompileResult::Fatal(f) => panic!("{:?}", f.diagnostics),
    };
    assert_eq!(desktop.design, mixos.read_projection());
    assert_eq!(desktop.accent, design::resolved_accent(&mixos));
    let today = crate::resolve(&Desktop::default()).unwrap();
    assert_ne!(desktop.design, today["desktop"].design);
}

/// Under an authored scheme the tints have no slot: they move nothing.
#[test]
fn tints_under_an_authored_scheme_change_nothing() {
    let studio = [("appearance.scheme", json!("studio"))];
    let base = crate::digest(&resolved(&values(&studio))).unwrap();
    let tinted = values(&[
        ("appearance.scheme", json!("studio")),
        ("appearance.window_bg", json!("#20242aff")),
        ("appearance.text_tint", json!("#c0ffee")),
    ]);
    assert_eq!(crate::digest(&resolved(&tinted)).unwrap(), base);
}

/// Under an authored scheme, a swatch resolves per mode and a custom colour
/// is the accent in both.
#[test]
fn the_sealed_accent_is_the_chosen_one() {
    for mode in ["dark", "light"] {
        let swatch = resolved(&values(&[
            ("appearance.scheme", json!("studio")),
            ("appearance.accent", json!("green")),
            ("appearance.mode", json!(mode)),
        ]));
        let green = design::Swatch::Green.colour(design::Mode::from_name(mode).unwrap());
        assert_eq!(
            swatch["desktop"].accent.map(|a| a.to_hex()),
            Some(green.to_hex()),
            "{mode}"
        );
    }
    let custom = resolved(&values(&[
        ("appearance.scheme", json!("studio")),
        ("appearance.accent", json!("#3a7f2c")),
    ]));
    for (context, e) in &custom {
        assert_eq!(
            e.accent.map(|a| a.to_hex()).as_deref(),
            Some("#3A7F2CFF"),
            "{context}"
        );
    }
}

/// The sealed accent and geometry survive the wire: a record reads back as
/// it was written.
#[test]
fn a_v2_effective_record_round_trips() {
    let next = resolved(&values(&[
        ("appearance.accent", json!("purple")),
        ("appearance.roundness", json!("slightly_round")),
        (
            "tune.style.radius",
            crate::registry::tests::tunable_sample("tune.style.radius"),
        ),
    ]));
    let text = serde_json::to_string(&next).unwrap();
    let back: BTreeMap<String, Effective> = serde_json::from_str(&text).unwrap();
    assert_eq!(back, next);
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

/// Each key v1 cannot express (the accent, a tint, the roundness and a
/// style tunable) changes the effective look of every context and its
/// digest, each alone, under the default `mixos`; the accent also changes
/// the sealed accent.
#[test]
fn seam_keys_change_the_effective_look_and_its_digest() {
    let base = resolved(&Values::new());
    let base_digest = crate::digest(&base).unwrap();
    for (key, value) in [
        ("appearance.accent", json!("purple")),
        ("appearance.window_bg", json!("#20242aff")),
        ("appearance.roundness", json!("square")),
        (
            "tune.style.radius",
            crate::registry::tests::tunable_sample("tune.style.radius"),
        ),
    ] {
        let one = values(&[(key, value)]);
        assert_eq!(one.len(), 1, "{key} is stored");
        let next = resolved(&one);
        assert_ne!(crate::digest(&next).unwrap(), base_digest, "{key}: digest");
        for (context, e) in &next {
            assert_ne!(e, &base[context], "{key}: {context}");
        }
        if key == "appearance.accent" {
            assert_ne!(next["desktop"].accent, base["desktop"].accent);
        }
    }
}
