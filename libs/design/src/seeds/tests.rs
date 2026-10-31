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
        parse_design_source(
            crate::SourceIdentity::new("embedded"),
            EMBEDDED_DEFAULT_SOURCE,
        )
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
    assert_eq!(semantic.len(), 12);
    assert!(!semantic.contains(&"palette.accent.default"));
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
    // Fable's R1: the desktop sits under the windows, shade over chrome.
    assert_eq!(dark["canvas"], [0x12, 0x12, 0x12, 255]);
    assert_eq!(dark["hover"], [0x43, 0x43, 0x43, 255]);
    assert_eq!(dark["pressed"], [0x58, 0x58, 0x58, 255]);
    assert_eq!(dark["text"], [0xe7, 0xe7, 0xe7, 255]);
    assert_eq!(dark["text_dim"], [0xc0, 0xc0, 0xc0, 255]);
    assert_eq!(dark["separator"], [0x44, 0x44, 0x44, 255]);
    assert_eq!(dark["accent"], [0x63, 0xd0, 0xdf, 255]);
    assert_eq!(dark["primary_text"], [0x03, 0x03, 0x03, 255]);
    assert_eq!(dark["scrim"], [0, 0, 0, 0x6e]);
    assert_eq!(dark["shadow"], [0, 0, 0, 0x52]);
    assert_eq!(dark["row_selected"], [0x63, 0xd0, 0xdf, 51]);
    let light = roles8(&compile(DesignContext {
        scheme: Scheme::Mixos,
        mode: Mode::Light,
        ..DesignContext::default()
    }));
    assert_eq!(light["chrome"], [0xd7, 0xd7, 0xd7, 255]);
    assert_eq!(light["card"], [0xeb, 0xeb, 0xeb, 255]);
    assert_eq!(light["canvas"], [0xc6, 0xc6, 0xc6, 255]);
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

/// Fable's C1: an achromatic scheme ties on chroma, and the tie goes to the
/// candidate that reads against `chrome`. Mono light takes its #222222
/// (not the #eeeeee beside its chrome); mono dark keeps #cecece; the
/// chromatic schemes keep their hue.
#[test]
fn seeds_of_takes_the_scheme_hue_and_breaks_ties_by_contrast() {
    let seed = |scheme, mode| {
        let design = compile(DesignContext {
            scheme,
            mode,
            ..DesignContext::default()
        });
        seeds_of(design.dictionary()).unwrap().accent.unwrap().to_srgba8()
    };
    assert_eq!(seed(Scheme::Mono, Mode::Light), [0x22, 0x22, 0x22, 255]);
    assert_eq!(seed(Scheme::Mono, Mode::Dark), [0xce, 0xce, 0xce, 255]);
    assert_eq!(seed(Scheme::Crimson, Mode::Dark), [0xf5, 0x2f, 0x39, 255]);
    assert_eq!(seed(Scheme::Ocean, Mode::Light), [0x00, 0x6f, 0x87, 255]);
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
    let mut cases: Vec<(AccentChoice, Tints)> = accents()
        .into_iter()
        .map(|a| (a, Tints::default()))
        .collect();
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

/// The roles of `design` as drawn: each composited over its own `chrome`.
fn drawn(design: &UnstampedResolvedDesign) -> BTreeMap<String, [u8; 4]> {
    let roles = crate::palette::roles_of(design.dictionary());
    let chrome = roles["chrome"];
    roles
        .iter()
        .map(|(role, colour)| {
            let colour = if colour.alpha < 1.0 {
                over(*colour, chrome)
            } else {
                *colour
            };
            (role.clone(), colour.to_srgba8())
        })
        .collect()
}

fn hue(colour: SrgbColour) -> f64 {
    crate::colour_model::derivation::linear_srgb_to_oklch(colour.to_linear()).2
}

/// F4's report: for every authored scheme, how far the first tint moves
/// the look, the authored roles against `mixos` from `seeds_of(roles)`,
/// both sides composited over their `chrome` before measuring (Fable's
/// ruling), so a transparent role measures as the chrome it shows. The
/// preserved set (`chrome`, `card`, `text`, the accent hue) is reported
/// apart. A recorded size, not a promised minimum. Run with `--nocapture`.
#[test]
fn role_distance_report() {
    println!("ROLE DISTANCE REPORT (8-bit steps, max channel, both sides over chrome)");
    println!(
        "{:<8} {:<5} {:<6} {:>6} {:>4} {:<20}  preserved: chrome card text  accent-hue°",
        "scheme", "mode", "contr", "mean", "max", "worst role"
    );
    let mut per_role: BTreeMap<&str, (u32, u8, u32)> = BTreeMap::new();
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
                let (a, b) = (drawn(&authored), drawn(&derived));
                let (worst, role, mean) = distance(&a, &b);
                for r in ROLES {
                    let d = delta(a[*r], b[*r]);
                    let entry = per_role.entry(r).or_default();
                    entry.0 += u32::from(d);
                    entry.1 = entry.1.max(d);
                    entry.2 += 1;
                }
                let seed_hue = seeds.accent.map_or(0.0, hue);
                let drawn_hue = hue(crate::palette::roles_of(derived.dictionary())["accent"]);
                let hue_delta = {
                    let d = (seed_hue - drawn_hue).rem_euclid(360.0);
                    d.min(360.0 - d)
                };
                println!(
                    "{:<8} {:<5} {:<6} {:>6.1} {:>4} {:<20}  {:>16} {:>4} {:>4}  {:>6.1}",
                    scheme.name(),
                    mode.name(),
                    contrast.name(),
                    mean,
                    worst,
                    role,
                    delta(a["chrome"], b["chrome"]),
                    delta(a["card"], b["card"]),
                    delta(a["text"], b["text"]),
                    hue_delta
                );
            }
        }
    }
    println!("PER ROLE over every case: mean, max");
    for role in ROLES {
        let (sum, max, n) = per_role[role];
        println!(
            "{role:<20} {:>6.1} {:>4}",
            f64::from(sum) / f64::from(n),
            max
        );
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

/// accent_soft fills selections under body text: `text` over `accent_soft`
/// over the scheme's `card` must keep AA for every swatch, in both modes.
/// The cases below 4.5:1 are printed (`--nocapture`) and pinned in
/// [`SOFT_BELOW_AA`], so a new one fails.
#[test]
fn body_text_reads_over_accent_soft_for_every_swatch() {
    let mut below = Vec::new();
    println!("ACCENT_SOFT CONTRAST (text over accent_soft over card), below 4.5:1");
    for scheme in Scheme::ALL {
        for mode in Mode::ALL {
            for swatch in Swatch::ALL {
                let design = compile(DesignContext {
                    scheme,
                    mode,
                    accent: AccentChoice::Swatch(swatch),
                    ..DesignContext::default()
                });
                let roles = crate::palette::roles_of(design.dictionary());
                let fill = over(roles["accent_soft"], roles["card"]);
                let ratio = crate::derive::primitives::contrast(roles["text"], fill);
                if ratio < 4.5 {
                    println!(
                        "{} {} {} {ratio:.2}",
                        scheme.name(),
                        mode.name(),
                        swatch.name()
                    );
                    below.push((scheme.name(), mode.name(), swatch.name()));
                }
            }
        }
    }
    assert_eq!(
        below, SOFT_BELOW_AA,
        "accent_soft cases below 4.5:1 changed"
    );
}

/// The swatch cases whose selection fill leaves body text under AA.
const SOFT_BELOW_AA: &[(&str, &str, &str)] = &[];

/// Fable's R5: whatever the text tint, `text` reads at least as well as
/// `text_dim`, and `text_dim` at least as well as `text_faint` drawn, all
/// against the background.
#[test]
fn the_text_rank_holds_for_every_text_tint() {
    let pick = |role: &str, theme: &DerivedTheme| {
        ROLE_MAP
            .iter()
            .find(|(name, _)| *name == role)
            .unwrap()
            .1
            .pick(theme)
    };
    let mut tints: Vec<SrgbColour> = Mode::ALL
        .into_iter()
        .flat_map(|mode| Swatch::ALL.map(|swatch| swatch.colour(mode)))
        .collect();
    for hex in ["#ffffff", "#000000", "#808080"] {
        tints.push(Colour8::from_hex(hex).unwrap().srgb());
    }
    for mode in Mode::ALL {
        for contrast in Contrast::ALL {
            for tint in &tints {
                let theme = derived_theme(&DesignContext {
                    scheme: Scheme::Mixos,
                    mode,
                    contrast,
                    tints: Tints {
                        text_tint: Some(Colour8(tint.to_srgba8())),
                        ..Tints::default()
                    },
                    ..DesignContext::default()
                });
                let base = theme.background.base;
                let reads = |colour: SrgbColour| {
                    crate::derive::primitives::contrast(over(colour, base), base)
                };
                let (text, dim, faint) = (
                    reads(pick("text", &theme)),
                    reads(pick("text_dim", &theme)),
                    reads(pick("text_faint", &theme)),
                );
                assert!(
                    text >= dim && dim >= faint,
                    "{mode:?} {contrast:?} tint {}: text {text:.2}, dim {dim:.2}, faint {faint:.2}",
                    tint.to_hex()
                );
            }
        }
    }
}
