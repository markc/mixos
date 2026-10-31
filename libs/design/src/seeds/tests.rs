// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use crate::family::chrome::ROLES;
use crate::{
    Contrast, DesignCompileResult, DesignContext, EMBEDDED_DEFAULT_SOURCE, Mode, Scheme, Swatch,
    UnstampedResolvedDesign, compile_design, parse_design_source,
};

fn document() -> &'static crate::DesignSourceDocument {
    static DOCUMENT: std::sync::OnceLock<crate::DesignSourceDocument> = std::sync::OnceLock::new();
    DOCUMENT.get_or_init(|| {
        parse_design_source(crate::SourceIdentity::new("embedded"), EMBEDDED_DEFAULT_SOURCE)
            .unwrap()
    })
}

fn compile(context: DesignContext) -> UnstampedResolvedDesign {
    match compile_design(document(), context.clone()) {
        DesignCompileResult::Success(success) => success.candidate,
        DesignCompileResult::Fatal(failure) => {
            panic!("{context:?} did not compile: {:#?}", failure.diagnostics)
        }
    }
}

fn roles8(design: &UnstampedResolvedDesign) -> BTreeMap<String, [u8; 4]> {
    crate::palette::roles_of(design.dictionary())
        .into_iter()
        .map(|(role, colour)| (role, colour.to_srgba8()))
        .collect()
}

fn delta(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap()
}

/// The largest per-channel difference of two role sets, with its role.
fn distance(a: &BTreeMap<String, [u8; 4]>, b: &BTreeMap<String, [u8; 4]>) -> (u8, String, f64) {
    let mut worst = (0, String::new());
    let mut sum = 0.0;
    for role in ROLES {
        let d = delta(a[*role], b[*role]);
        sum += f64::from(d);
        if d > worst.0 {
            worst = (d, (*role).to_owned());
        }
    }
    (worst.0, worst.1, sum / ROLES.len() as f64)
}

fn accents() -> Vec<AccentChoice> {
    let mut out = vec![AccentChoice::Auto];
    out.extend(Swatch::ALL.map(AccentChoice::Swatch));
    for hex in ["#000000", "#ffffff", "#808080", "#ff00ff"] {
        out.push(AccentChoice::Custom(Colour8::from_hex(hex).unwrap()));
    }
    out
}

#[test]
fn the_role_map_covers_every_chrome_role_in_order() {
    assert_eq!(ROLE_MAP.map(|(role, _)| role), ROLES);
    let semantic: Vec<_> = SEMANTIC_MAP.iter().map(|(name, _)| *name).collect();
    assert_eq!(semantic.len(), 13);
}

#[test]
fn every_scheme_mode_contrast_and_accent_compiles() {
    for scheme in Scheme::ALL {
        for mode in Mode::ALL {
            for contrast in Contrast::ALL {
                for accent in accents() {
                    let design = compile(DesignContext {
                        scheme,
                        mode,
                        contrast,
                        accent,
                        ..DesignContext::default()
                    });
                    let roles = roles8(&design);
                    assert_eq!(roles.len(), 31, "{scheme:?} {mode:?}");
                    if let Some(colour) = accent.resolve(mode) {
                        assert_eq!(
                            roles["accent"],
                            colour.to_srgba8(),
                            "{scheme:?} {mode:?} {accent:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn tints_compile_under_mixos_and_are_ignored_by_authored_schemes() {
    let tints = Tints {
        window_bg: Colour8::from_hex("#20252b"),
        container_bg: Colour8::from_hex("#2b3038"),
        text_tint: Colour8::from_hex("#e8f0ff"),
        control_tint: Colour8::from_hex("#3366aa"),
    };
    for mode in Mode::ALL {
        let plain = compile(DesignContext {
            scheme: Scheme::Studio,
            mode,
            ..DesignContext::default()
        });
        let tinted = compile(DesignContext {
            scheme: Scheme::Studio,
            mode,
            tints,
            ..DesignContext::default()
        });
        assert_eq!(roles8(&plain), roles8(&tinted));
        let mixos = compile(DesignContext {
            scheme: Scheme::Mixos,
            mode,
            tints,
            ..DesignContext::default()
        });
        assert_eq!(roles8(&mixos)["chrome"], [0x20, 0x25, 0x2b, 255]);
        assert_eq!(roles8(&mixos)["card"], [0x2b, 0x30, 0x38, 255]);
    }
}

#[test]
fn mixos_draws_the_reference_numbers() {
    // Spec §4.5's dark and light defaults, through the compile.
    let dark = roles8(&compile(DesignContext {
        scheme: Scheme::Mixos,
        mode: Mode::Dark,
        ..DesignContext::default()
    }));
    assert_eq!(dark["chrome"], [0x1b, 0x1b, 0x1b, 255]);
    assert_eq!(dark["card"], [0x27, 0x27, 0x27, 255]);
    assert_eq!(dark["canvas"], [0x34, 0x34, 0x34, 255]);
    assert_eq!(dark["hover"], [0x43, 0x43, 0x43, 255]);
    assert_eq!(dark["pressed"], [0x58, 0x58, 0x58, 255]);
    assert_eq!(dark["text"], [0xe7, 0xe7, 0xe7, 255]);
    assert_eq!(dark["text_dim"], [0xc0, 0xc0, 0xc0, 255]);
    assert_eq!(dark["separator"], [0x44, 0x44, 0x44, 255]);
    assert_eq!(dark["accent"], [0x63, 0xd0, 0xdf, 255]);
    assert_eq!(dark["primary_text"], [0x03, 0x03, 0x03, 255]);
    assert_eq!(dark["scrim"], [0, 0, 0, 0x52]);
    let light = roles8(&compile(DesignContext {
        scheme: Scheme::Mixos,
        mode: Mode::Light,
        ..DesignContext::default()
    }));
    assert_eq!(light["chrome"], [0xd7, 0xd7, 0xd7, 255]);
    assert_eq!(light["card"], [0xeb, 0xeb, 0xeb, 255]);
    assert_eq!(light["text"], [0x12, 0x12, 0x12, 255]);
    assert_eq!(light["accent"], [0x00, 0x52, 0x5a, 255]);
    assert_eq!(light["primary_text"], [0xe1, 0xe1, 0xe1, 255]);
}

#[test]
fn a_grey_accent_takes_the_lifted_accent_text() {
    // Spec §4.5's grey accent: under 4:1 on the primary container, the
    // accent text walks the accent's ramp to #B6B6B6.
    let grey = AccentChoice::Custom(Colour8::from_hex("#7f7f7f").unwrap());
    let mixos = roles8(&compile(DesignContext {
        scheme: Scheme::Mixos,
        mode: Mode::Dark,
        accent: grey,
        ..DesignContext::default()
    }));
    assert_eq!(mixos["accent_text"], [0xb6, 0xb6, 0xb6, 255]);
    // The same rule under an authored scheme, against its own card.
    let studio = roles8(&compile(DesignContext {
        scheme: Scheme::Studio,
        mode: Mode::Dark,
        accent: grey,
        ..DesignContext::default()
    }));
    assert_eq!(studio["accent"], [0x7f, 0x7f, 0x7f, 255]);
    assert_eq!(studio["accent_border"], [0x7f, 0x7f, 0x7f, 255]);
    assert_eq!(studio["accent_soft"], [0x7f, 0x7f, 0x7f, 51]);
    let colour = |role: &str| {
        let [r, g, b, a] = studio[role].map(|c| f64::from(c) / 255.0);
        SrgbColour {
            red: r,
            green: g,
            blue: b,
            alpha: a,
        }
    };
    let expected = crate::derive::accent_text_for(
        colour("accent"),
        colour("chrome"),
        colour("card"),
        Mode::Dark,
        Contrast::Normal,
    )
    .unwrap_or(colour("accent"));
    assert_eq!(studio["accent_text"], expected.to_srgba8());
}

#[test]
fn auto_keeps_every_authored_scheme_unchanged() {
    // The axes default to nothing: an authored scheme at `auto` compiles
    // as it did before the axes existed, in every role.
    let document = document();
    for scheme in Scheme::AUTHORED {
        for mode in Mode::ALL {
            let context = DesignContext {
                scheme,
                mode,
                ..DesignContext::default()
            };
            let design = compile(context.clone());
            let mut flattened = document.v1.clone();
            let mut authored = BTreeSet::new();
            assert!(
                complete_source(&mut flattened, &mut authored, &context, Semantics::Seeds)
                    .is_empty()
            );
            assert!(authored.is_empty());
            assert_eq!(roles8(&design).len(), 31);
        }
    }
}

/// The F4 claim for `mixos`: the seeds read back from its roles re-derive
/// the same 31 roles within one 8-bit step per channel.
#[test]
fn seeds_of_round_trips_mixos_within_one_step() {
    let tinted = Tints {
        window_bg: Colour8::from_hex("#202020"),
        container_bg: Colour8::from_hex("#2a2a2a"),
        text_tint: None,
        control_tint: None,
    };
    let mut cases: Vec<(AccentChoice, Tints)> =
        accents().into_iter().map(|a| (a, Tints::default())).collect();
    cases.push((AccentChoice::Auto, tinted));
    for mode in Mode::ALL {
        for contrast in Contrast::ALL {
            for (accent, tints) in &cases {
                let context = DesignContext {
                    scheme: Scheme::Mixos,
                    mode,
                    contrast,
                    accent: *accent,
                    tints: *tints,
                    ..DesignContext::default()
                };
                let first = compile(context.clone());
                let seeds = seeds_of(first.dictionary()).unwrap();
                let (accent, tints) = tint_batch(&seeds);
                let second = compile(DesignContext {
                    accent,
                    tints,
                    ..context.clone()
                });
                let (worst, role, _) = distance(&roles8(&first), &roles8(&second));
                assert!(
                    worst <= 1,
                    "{context:?}: {role} moved {worst} steps after the round trip"
                );
            }
        }
    }
}

/// F4's report: for every authored scheme, how far the first tint moves
/// the look, the authored roles against `mixos` from `seeds_of(roles)`.
/// Run with `--nocapture` to read it.
#[test]
fn role_distance_report() {
    println!("ROLE DISTANCE REPORT (8-bit steps, max channel; authored vs mixos from seeds_of)");
    println!(
        "{:<8} {:<5} {:<6} {:>4} {:<20} {:>6}  three largest",
        "scheme", "mode", "contr", "max", "worst role", "mean"
    );
    for scheme in Scheme::AUTHORED {
        for mode in Mode::ALL {
            for contrast in Contrast::ALL {
                let context = DesignContext {
                    scheme,
                    mode,
                    contrast,
                    ..DesignContext::default()
                };
                let authored = compile(context.clone());
                let seeds = seeds_of(authored.dictionary()).unwrap();
                let (accent, tints) = tint_batch(&seeds);
                let derived = compile(DesignContext {
                    scheme: Scheme::Mixos,
                    accent,
                    tints,
                    ..context
                });
                let (a, b) = (roles8(&authored), roles8(&derived));
                let (worst, role, mean) = distance(&a, &b);
                let mut each: Vec<_> = ROLES.iter().map(|r| (delta(a[*r], b[*r]), *r)).collect();
                each.sort_by(|x, y| y.cmp(x));
                let top: Vec<_> = each
                    .iter()
                    .take(3)
                    .map(|(d, r)| format!("{r} {d}"))
                    .collect();
                println!(
                    "{:<8} {:<5} {:<6} {:>4} {:<20} {:>6.1}  {}",
                    scheme.name(),
                    mode.name(),
                    contrast.name(),
                    worst,
                    role,
                    mean,
                    top.join(", ")
                );
            }
        }
    }
}

#[test]
fn the_role_map_reads_as_a_table() {
    // The table Fable signs off, printed with `--nocapture`.
    println!("ROLE MAP");
    for (role, source) in ROLE_MAP {
        println!("{role:<20} <- {}", source.describe());
    }
    println!("SEMANTIC MAP");
    for (name, source) in SEMANTIC_MAP {
        println!("{name:<28} <- {}", source.describe());
    }
    assert_eq!(ROLE_MAP[0].1.describe(), "background.base");
    assert_eq!(
        ROLE_MAP[8].1.describe(),
        "button.divider over button.base over primary.base"
    );
}
