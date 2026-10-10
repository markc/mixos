//! The chrome family: the exact colours of application chrome (title bars,
//! menus, tabs, panels, controls) as one closed set of roles, each naming a
//! colour primitive.
//!
//! A scheme with a chrome palette (pro, studio, classic) authors each role's
//! primitive in its modifier blocks, and the role renders exactly as
//! authored: no contrast check or recipe applies, and written as OKLCH at
//! nine decimals every 8-bit sRGB value round-trips (see
//! `colour_model::exact_authoring_tests`). Any other palette derives its
//! chrome from its semantic pairs through `derive`, so every scheme can take
//! a chrome style. Per role, a primitive a selected block authors wins, then
//! the derivation, then the primitive's base declaration.
//!
//! The shipped derivation (the source's `derive` map):
//!
//! | role | from |
//! |---|---|
//! | `chrome`, `field` | `base` surface |
//! | `text`, `icon` | `base` foreground |
//! | `text_dim`, `text_faint` | `base` foreground 8% and 18% toward its surface |
//! | `card` | `secondary` surface |
//! | `hover`, `pressed` | `secondary` surface 8% and 16% toward its foreground |
//! | `canvas`, `dock`, `tab_strip` | `muted` surface |
//! | `card_border`, `field_border`, `separator`, `canvas_dot` | `border` |
//! | `primary_bg` | `primary` surface |
//! | `primary_text` | `primary` foreground |
//! | `accent`, `accent_soft`, `row_selected`, `menu_highlight` | `accent` surface |
//! | `accent_text`, `menu_highlight_text` | `accent` foreground |
//! | `accent_border` | `ring` |
//! | `danger`, `caption_close` | `destructive` surface |
//! | `caption_close_text` | `destructive` foreground |
//! | `warning` | primitive `status.warning` |
//! | `shadow`, `scrim` | primitives `chrome.shade.shadow`, `chrome.shade.scrim` |
//!
//! Text roles sit on surfaces of the same pair or, mixed toward that pair's
//! surface, keep the pairs' WCAG AA floor (a test checks every hue palette).
//! `accent` is the tinted accent pair, not the strong primary: a chrome
//! style fills selections with it under the body text.
//!
//! Source shape, under `design.v1.families`:
//!
//! ```text
//! chrome: {
//!   coverage: "explicit",
//!   roles: { "panel": "chrome.panel", ... },
//!   derive: { panel: { kind: "pair", pair: "base", part: "surface" }, ... }
//! }
//! ```
//!
//! Coverage `explicit` makes a missing role an error; the default `warn`
//! reports it and leaves the role out. An unknown role or primitive, or a
//! derivation naming an unknown pair, non-text colour or primitive, is always
//! an error, in every context. A derivation is `pair` (a pair's surface or
//! foreground), `non_text`, `primitive`, or `mix` (one part of a pair moved
//! toward the other by `amount`, in linear light).

use crate::source::{
    ChromeDeriveSource, ChromeMappingSource, CoveragePolicy, DesignV1Source, PairPart,
};
use crate::{DesignDiagnostic, LinearRgba, ResolvedColours};
use std::collections::{BTreeMap, BTreeSet};

const PATH: &str = "design.v1.families.chrome";

/// Every chrome role. The vocabulary is shared with the desktop-editor
/// chrome MixOS matches, name for name, so a value change there is a one-row
/// change here. A role is added here first, then authored in each design.
pub const ROLES: &[&str] = &[
    // Surfaces.
    "chrome",
    "canvas",
    "canvas_dot",
    "dock",
    "card",
    "card_border",
    "tab_strip",
    // Controls and their states.
    "field",
    "field_border",
    "hover",
    "pressed",
    "row_selected",
    // Text and icons.
    "text",
    "text_dim",
    "text_faint",
    "icon",
    // Accent and selection.
    "accent",
    "accent_soft",
    "accent_border",
    "accent_text",
    "menu_highlight",
    "menu_highlight_text",
    // Lines, depth and overlays.
    "separator",
    "shadow",
    "scrim",
    // Actions and status.
    "primary_bg",
    "primary_text",
    "danger",
    "warning",
    "caption_close",
    "caption_close_text",
];

/// The resolved chrome colours: role name to its exact colour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedChrome {
    colours: BTreeMap<String, LinearRgba>,
}

impl ResolvedChrome {
    /// The colour of `role`, when the design authors it.
    pub fn get(&self, role: &str) -> Option<LinearRgba> {
        self.colours.get(role).copied()
    }

    /// The authored roles and their colours, in role order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, LinearRgba)> {
        self.colours
            .iter()
            .map(|(role, colour)| (role.as_str(), *colour))
    }
}

/// What a chrome family compiles against: the context's resolved colours,
/// and the colour primitives a selected modifier block authors (as opposed
/// to the base's declarations).
pub(crate) struct Inputs<'a> {
    pub colours: &'a ResolvedColours,
    pub authored: &'a BTreeSet<String>,
}

/// Compile the chrome family for one context. `None` when the design authors
/// no chrome family. Errors are fatal; warnings ride along.
pub(crate) fn compile(
    source: &DesignV1Source,
    inputs: &Inputs<'_>,
) -> Result<(Option<ResolvedChrome>, Vec<DesignDiagnostic>), Vec<DesignDiagnostic>> {
    compile_roles(source.families.chrome.as_ref(), inputs, ROLES)
}

/// One derivation's colour, or why it has none.
fn derived(
    expression: &ChromeDeriveSource,
    colours: &ResolvedColours,
) -> Result<LinearRgba, String> {
    let pair = |name: &str| {
        colours
            .pairs
            .get(name)
            .ok_or_else(|| format!("`{name}` is not a semantic pair"))
    };
    let part = |pair: &crate::ResolvedPair, part: PairPart| match part {
        PairPart::Surface => pair.rendered_surface,
        PairPart::Foreground => pair.rendered_foreground,
    };
    match expression {
        ChromeDeriveSource::Pair {
            pair: name,
            part: which,
        } => Ok(part(pair(name)?, *which)),
        ChromeDeriveSource::NonText { value } => colours
            .non_text
            .get(value)
            .map(|colour| colour.value)
            .ok_or_else(|| format!("`{value}` is not a non-text colour")),
        ChromeDeriveSource::Primitive { value } => colours
            .primitives
            .get(value)
            .copied()
            .ok_or_else(|| format!("`{value}` is not a colour primitive")),
        ChromeDeriveSource::Mix {
            pair: name,
            from,
            toward,
            amount,
            min_contrast,
            ..
        } => {
            if !(0.0..=1.0).contains(amount) {
                return Err(format!("amount {amount} is outside 0 to 1"));
            }
            if min_contrast.is_some_and(|m| !(1.0..=21.0).contains(&m)) {
                return Err("min_contrast is outside 1 to 21".into());
            }
            let pair = pair(name)?;
            Ok(mix(part(pair, *from), part(pair, *toward), *amount))
        }
    }
}

/// `a` moved toward `b` by `t`, in linear light.
fn mix(a: LinearRgba, b: LinearRgba, t: f64) -> LinearRgba {
    let lerp = |x: f64, y: f64| x + (y - x) * t;
    LinearRgba {
        red: lerp(a.red, b.red),
        green: lerp(a.green, b.green),
        blue: lerp(a.blue, b.blue),
        alpha: lerp(a.alpha, b.alpha),
    }
}

/// A floored mix (`min_contrast` with `against`): the largest move up to its
/// `amount` that keeps at least `min_contrast` with every background, both
/// measured as an 8-bit sRGB renderer draws them, or why none does. Contrast
/// falls as the move grows, so a bisection finds it.
fn floored(
    expression: &ChromeDeriveSource,
    colours: &ResolvedColours,
    backgrounds: &[LinearRgba],
) -> Result<LinearRgba, String> {
    let ChromeDeriveSource::Mix {
        pair,
        from,
        toward,
        amount,
        min_contrast: Some(floor),
        ..
    } = expression
    else {
        unreachable!("only floored mixes are deferred");
    };
    let pair = &colours.pairs[pair];
    let part = |part: &PairPart| match part {
        PairPart::Surface => pair.rendered_surface,
        PairPart::Foreground => pair.rendered_foreground,
    };
    let (a, b) = (part(from), part(toward));
    // Measured as drawn: both colours quantised to 8-bit sRGB.
    let clears = |t: f64| {
        let text = mix(a, b, t).rendered();
        backgrounds
            .iter()
            .all(|bg| crate::contrast_ratio(text, bg.rendered()) >= *floor)
    };
    if clears(*amount) {
        return Ok(mix(a, b, *amount));
    }
    if !clears(0.0) {
        return Err(format!(
            "even unmixed, the colour is under {floor}:1 with a background"
        ));
    }
    let (mut low, mut high) = (0.0, *amount);
    for _ in 0..40 {
        let mid = (low + high) / 2.0;
        if clears(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Ok(mix(a, b, low))
}

fn is_floored(expression: &ChromeDeriveSource) -> bool {
    matches!(
        expression,
        ChromeDeriveSource::Mix {
            min_contrast: Some(_),
            ..
        }
    )
}

/// Each role resolves, in order, to: its primitive when a selected modifier
/// block authors it (the scheme's own chrome colour); else its derivation
/// from the palette; else its primitive's base declaration. A design with
/// no derivations therefore keeps every authored value, base or block.
fn compile_roles(
    family: Option<&ChromeMappingSource>,
    inputs: &Inputs<'_>,
    roles: &[&str],
) -> Result<(Option<ResolvedChrome>, Vec<DesignDiagnostic>), Vec<DesignDiagnostic>> {
    let Some(family) = family else {
        return Ok((None, Vec::new()));
    };
    let primitives = &inputs.colours.primitives;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut colours = BTreeMap::new();
    // Every derivation is checked whether or not this context uses it, so a
    // design that compiles for one scheme compiles for all.
    let mut derivations = BTreeMap::new();
    for (role, expression) in &family.derive {
        let path = format!("{PATH}.derive.{role}");
        if !roles.contains(&role.as_str()) {
            errors.push(DesignDiagnostic::error(
                "unknown-chrome-role",
                path,
                format!("`{role}` is not a chrome role"),
            ));
            continue;
        }
        match derived(expression, inputs.colours) {
            Ok(colour) => {
                derivations.insert(role.as_str(), colour);
            }
            Err(message) => errors.push(DesignDiagnostic::error(
                "invalid-chrome-derivation",
                path,
                message,
            )),
        }
    }
    for (role, primitive) in &family.roles {
        let path = format!("{PATH}.roles.{role}");
        if !roles.contains(&role.as_str()) {
            errors.push(DesignDiagnostic::error(
                "unknown-chrome-role",
                path,
                format!("`{role}` is not a chrome role"),
            ));
            continue;
        }
        let authored = inputs.authored.contains(primitive);
        match (primitives.get(primitive), derivations.get(role.as_str())) {
            (Some(colour), _) if authored => {
                colours.insert(role.clone(), *colour);
            }
            (_, Some(colour)) => {
                colours.insert(role.clone(), *colour);
            }
            (Some(colour), None) => {
                colours.insert(role.clone(), *colour);
            }
            (None, None) => errors.push(DesignDiagnostic::error(
                "unknown-primitive",
                path,
                format!("`{primitive}` is not a colour primitive"),
            )),
        }
    }
    for (role, colour) in &derivations {
        if !family.roles.contains_key(*role) {
            colours.insert((*role).to_owned(), *colour);
        }
    }
    // A floored mix takes its colour from the backgrounds it was resolved
    // against, where the role came from its derivation and not a block.
    for (role, expression) in family.derive.iter().filter(|(_, e)| is_floored(e)) {
        let path = format!("{PATH}.derive.{role}");
        let ChromeDeriveSource::Mix { against, .. } = expression else {
            continue;
        };
        let authored = family
            .roles
            .get(role)
            .is_some_and(|p| inputs.authored.contains(p));
        if authored || !derivations.contains_key(role.as_str()) {
            continue;
        }
        let mut backgrounds = Vec::new();
        for background in against {
            match (colours.get(background), family.derive.get(background)) {
                (_, Some(e)) if is_floored(e) => errors.push(DesignDiagnostic::error(
                    "invalid-chrome-derivation",
                    path.clone(),
                    format!("`{background}` is itself floored and cannot be a background"),
                )),
                (Some(colour), _) => backgrounds.push(*colour),
                (None, _) => errors.push(DesignDiagnostic::error(
                    "invalid-chrome-derivation",
                    path.clone(),
                    format!("`{background}` is not a resolved chrome role"),
                )),
            }
        }
        if against.is_empty() {
            errors.push(DesignDiagnostic::error(
                "invalid-chrome-derivation",
                path.clone(),
                "min_contrast needs the backgrounds it is `against`",
            ));
            continue;
        }
        match floored(expression, inputs.colours, &backgrounds) {
            Ok(colour) => {
                colours.insert(role.clone(), colour);
            }
            Err(message) => errors.push(DesignDiagnostic::error(
                "chrome-derivation-contrast",
                path,
                message,
            )),
        }
    }
    for role in roles.iter().filter(|role| !colours.contains_key(**role)) {
        if family.roles.contains_key(*role) || family.derive.contains_key(*role) {
            continue;
        }
        let path = format!("{PATH}.roles.{role}");
        let message = format!("chrome role `{role}` is not authored");
        match family.coverage {
            CoveragePolicy::Explicit => errors.push(DesignDiagnostic::error(
                "missing-chrome-role",
                path,
                message,
            )),
            CoveragePolicy::Warn => warnings.push(DesignDiagnostic::warning(
                "missing-chrome-role",
                path,
                message,
            )),
        }
    }
    if errors.is_empty() {
        Ok((Some(ResolvedChrome { colours }), warnings))
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: &[&str] = &["panel", "text"];

    fn source(coverage: CoveragePolicy, roles: &[(&str, &str)]) -> ChromeMappingSource {
        ChromeMappingSource {
            coverage,
            roles: roles
                .iter()
                .map(|(r, p)| ((*r).to_owned(), (*p).to_owned()))
                .collect(),
            derive: BTreeMap::new(),
        }
    }

    /// Compile `family` over [`primitives`], with `authored` set by a block.
    fn run(
        family: Option<&ChromeMappingSource>,
        authored: &[&str],
    ) -> Result<(Option<ResolvedChrome>, Vec<DesignDiagnostic>), Vec<DesignDiagnostic>> {
        let colours = ResolvedColours {
            primitives: primitives(),
            ..Default::default()
        };
        let authored = authored.iter().map(|p| (*p).to_owned()).collect();
        compile_roles(
            family,
            &Inputs {
                colours: &colours,
                authored: &authored,
            },
            ROLES,
        )
    }

    fn primitives() -> BTreeMap<String, LinearRgba> {
        BTreeMap::from([
            ("p.panel".into(), LinearRgba::BLACK),
            ("p.text".into(), LinearRgba::WHITE),
        ])
    }

    #[test]
    fn roles_resolve_to_their_primitives_exactly() {
        let s = source(
            CoveragePolicy::Explicit,
            &[("panel", "p.panel"), ("text", "p.text")],
        );
        let (chrome, warnings) = run(Some(&s), &[]).unwrap();
        let chrome = chrome.unwrap();
        assert_eq!(chrome.get("panel"), Some(LinearRgba::BLACK));
        assert_eq!(chrome.get("text"), Some(LinearRgba::WHITE));
        assert!(warnings.is_empty());
    }

    #[test]
    fn explicit_coverage_refuses_a_missing_role_and_warn_reports_it() {
        let explicit = source(CoveragePolicy::Explicit, &[("panel", "p.panel")]);
        let errors = run(Some(&explicit), &[]).unwrap_err();
        assert_eq!(errors[0].code, "missing-chrome-role");
        let warn = source(CoveragePolicy::Warn, &[("panel", "p.panel")]);
        let (chrome, warnings) = run(Some(&warn), &[]).unwrap();
        assert_eq!(warnings[0].code, "missing-chrome-role");
        assert_eq!(chrome.unwrap().get("text"), None);
    }

    #[test]
    fn unknown_roles_and_primitives_are_errors() {
        let s = source(
            CoveragePolicy::Warn,
            &[("panel", "nope"), ("bogus", "p.text"), ("text", "p.text")],
        );
        let codes: Vec<_> = run(Some(&s), &[])
            .unwrap_err()
            .iter()
            .map(|d| d.code)
            .collect();
        assert!(
            codes.contains(&"unknown-primitive") && codes.contains(&"unknown-chrome-role"),
            "{codes:?}"
        );
    }

    #[test]
    fn a_design_without_chrome_has_none() {
        assert_eq!(run(None, &[]).unwrap().0, None);
    }

    /// The embedded design renders the chrome exactly as measured, in every
    /// chrome scheme and mode (values from the chrome specification).
    #[test]
    fn the_embedded_chrome_schemes_render_the_measured_values() {
        use crate::{DesignCompileResult, DesignContext, Mode, Scheme, SourceIdentity};
        let document = crate::parse_design_source(
            SourceIdentity::new("embedded"),
            crate::EMBEDDED_DEFAULT_SOURCE,
        )
        .expect("embedded design parses");
        let hex = |c: LinearRgba| {
            let [r, g, b, a] = c.to_srgba8();
            format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
        };
        // (scheme, mode) -> [chrome, card, text, accent, shadow] as #RRGGBBAA.
        let expected = [
            (
                Scheme::Pro,
                Mode::Dark,
                [
                    "#323232FF",
                    "#323232FF",
                    "#DEDEDEFF",
                    "#378EF0FF",
                    "#00000096",
                ],
            ),
            (
                Scheme::Pro,
                Mode::Light,
                [
                    "#535353FF",
                    "#535353FF",
                    "#EEEEEEFF",
                    "#378EF0FF",
                    "#00000096",
                ],
            ),
            (
                Scheme::Studio,
                Mode::Dark,
                [
                    "#141415FF",
                    "#1A1A1CFF",
                    "#ECECF0FF",
                    "#8B7CF6FF",
                    "#0000008C",
                ],
            ),
            (
                Scheme::Studio,
                Mode::Light,
                [
                    "#F6F6F8FF",
                    "#FCFCFDFF",
                    "#18181CFF",
                    "#6C5CE7FF",
                    "#00000032",
                ],
            ),
            (
                Scheme::Classic,
                Mode::Dark,
                [
                    "#D4D0C8FF",
                    "#D4D0C8FF",
                    "#000000FF",
                    "#0A246AFF",
                    "#00000000",
                ],
            ),
            (
                Scheme::Classic,
                Mode::Light,
                [
                    "#D4D0C8FF",
                    "#D4D0C8FF",
                    "#000000FF",
                    "#0A246AFF",
                    "#00000000",
                ],
            ),
        ];
        for (scheme, mode, values) in expected {
            let context = DesignContext {
                scheme,
                mode,
                ..DesignContext::default()
            };
            let DesignCompileResult::Success(success) = crate::compile_design(&document, context)
            else {
                panic!("{scheme:?}/{mode:?} must compile");
            };
            let chrome = success
                .candidate
                .dictionary()
                .chrome
                .clone()
                .expect("chrome family");
            for (role, want) in ["chrome", "card", "text", "accent", "shadow"]
                .into_iter()
                .zip(values)
            {
                let got = chrome.get(role).map(hex);
                assert_eq!(got.as_deref(), Some(want), "{scheme:?}/{mode:?} {role}");
            }
            assert_eq!(
                chrome.iter().count(),
                super::ROLES.len(),
                "{scheme:?}/{mode:?}: every role authored"
            );
        }
    }

    fn embedded() -> crate::DesignSourceDocument {
        crate::parse_design_source(
            crate::SourceIdentity::new("embedded"),
            crate::EMBEDDED_DEFAULT_SOURCE,
        )
        .expect("embedded design parses")
    }

    fn compiled(
        document: &crate::DesignSourceDocument,
        context: crate::DesignContext,
    ) -> crate::UnstampedResolvedDesign {
        match crate::compile_design(document, context.clone()) {
            crate::DesignCompileResult::Success(success) => success.candidate,
            crate::DesignCompileResult::Fatal(failure) => {
                panic!("{context:?} must compile: {:?}", failure.diagnostics)
            }
        }
    }

    /// Every scheme, in every style (its own and the four), mode and
    /// contrast, compiles with all its chrome roles and a style.
    #[test]
    fn every_scheme_style_mode_and_contrast_compiles() {
        use crate::{Contrast, DesignContext, Mode, Scheme, Style};
        let document = embedded();
        let styles = std::iter::once(None).chain(Style::ALL.into_iter().map(Some));
        for style in styles {
            for scheme in Scheme::ALL {
                for mode in Mode::ALL {
                    for contrast in Contrast::ALL {
                        let context = DesignContext {
                            scheme,
                            mode,
                            contrast,
                            style,
                            app: None,
                        };
                        let design = compiled(&document, context);
                        let dictionary = design.dictionary();
                        let chrome = dictionary.chrome.as_ref().expect("chrome family");
                        assert_eq!(
                            chrome.iter().count(),
                            super::ROLES.len(),
                            "{scheme:?} {style:?} {mode:?} {contrast:?}"
                        );
                        assert!(dictionary.style.is_some());
                    }
                }
            }
        }
    }

    /// What each derived role is made of, as the module documents it.
    enum Made {
        Pair(&'static str, PairPart),
        NonText(&'static str),
        Primitive(&'static str),
        Mix(&'static str, PairPart, PairPart, f64),
    }

    const DOCUMENTED: &[(&str, Made)] = {
        use Made::{Mix, NonText, Pair, Primitive};
        use PairPart::{Foreground as Fg, Surface as Bg};
        &[
            ("chrome", Pair("base", Bg)),
            ("canvas", Pair("muted", Bg)),
            ("canvas_dot", NonText("border")),
            ("dock", Pair("muted", Bg)),
            ("card", Pair("secondary", Bg)),
            ("card_border", NonText("border")),
            ("tab_strip", Pair("muted", Bg)),
            ("field", Pair("base", Bg)),
            ("field_border", NonText("border")),
            ("hover", Mix("secondary", Bg, Fg, 0.08)),
            ("pressed", Mix("secondary", Bg, Fg, 0.16)),
            ("row_selected", Pair("accent", Bg)),
            ("text", Pair("base", Fg)),
            ("text_dim", Mix("base", Fg, Bg, 0.08)),
            ("text_faint", Mix("base", Fg, Bg, 0.18)),
            ("icon", Pair("base", Fg)),
            ("accent", Pair("accent", Bg)),
            ("accent_soft", Pair("accent", Bg)),
            ("accent_border", NonText("ring")),
            ("accent_text", Pair("accent", Fg)),
            ("menu_highlight", Pair("accent", Bg)),
            ("menu_highlight_text", Pair("accent", Fg)),
            ("separator", NonText("border")),
            ("shadow", Primitive("chrome.shade.shadow")),
            ("scrim", Primitive("chrome.shade.scrim")),
            ("primary_bg", Pair("primary", Bg)),
            ("primary_text", Pair("primary", Fg)),
            ("danger", Pair("destructive", Bg)),
            ("warning", Primitive("status.warning")),
            ("caption_close", Pair("destructive", Bg)),
            ("caption_close_text", Pair("destructive", Fg)),
        ]
    };

    /// The dimmed text roles, floored at AA against [`BACKGROUNDS`].
    const FLOORED: [&str; 2] = ["text_dim", "text_faint"];

    /// The chrome surfaces text is drawn on.
    const BACKGROUNDS: [&str; 7] = [
        "chrome",
        "card",
        "field",
        "tab_strip",
        "dock",
        "canvas",
        "hover",
    ];

    /// A hue palette's chrome is its pairs, role by role, as documented; the
    /// table covers every role.
    #[test]
    fn hue_palettes_derive_their_chrome_as_documented() {
        use crate::{Contrast, DesignContext, Mode, Scheme};
        assert_eq!(DOCUMENTED.len(), super::ROLES.len());
        let document = embedded();
        for scheme in Scheme::REVISION_ONE {
            for mode in Mode::ALL {
                for contrast in Contrast::ALL {
                    let context = DesignContext {
                        scheme,
                        mode,
                        contrast,
                        ..DesignContext::default()
                    };
                    let design = compiled(&document, context);
                    let colours = &design.dictionary().colours;
                    let chrome = design.dictionary().chrome.clone().unwrap();
                    let part = |pair: &str, part: &PairPart| {
                        let pair = &colours.pairs[pair];
                        match part {
                            PairPart::Surface => pair.rendered_surface,
                            PairPart::Foreground => pair.rendered_foreground,
                        }
                    };
                    for (role, from) in DOCUMENTED {
                        let want = match from {
                            Made::Pair(pair, which) => part(pair, which),
                            Made::NonText(name) => colours.non_text[*name].value,
                            Made::Primitive(name) => colours.primitives[*name],
                            Made::Mix(pair, a, b, t) => {
                                let (a, b) = (part(pair, a), part(pair, b));
                                let lerp = |x: f64, y: f64| x + (y - x) * t;
                                LinearRgba {
                                    red: lerp(a.red, b.red),
                                    green: lerp(a.green, b.green),
                                    blue: lerp(a.blue, b.blue),
                                    alpha: lerp(a.alpha, b.alpha),
                                }
                            }
                        };
                        let got = chrome.get(role).unwrap();
                        if FLOORED.contains(role) {
                            let clears = |c: LinearRgba| {
                                BACKGROUNDS.iter().all(|bg| {
                                    crate::contrast_ratio(
                                        c.rendered(),
                                        chrome.get(bg).unwrap().rendered(),
                                    ) >= 4.5
                                })
                            };
                            if clears(want) {
                                assert_eq!(
                                    got, want,
                                    "{scheme:?}/{mode:?}/{contrast:?} {role}: the full mix clears"
                                );
                            } else {
                                assert!(
                                    clears(got),
                                    "{scheme:?}/{mode:?}/{contrast:?} {role}: floored"
                                );
                            }
                            continue;
                        }
                        assert_eq!(got, want, "{scheme:?}/{mode:?}/{contrast:?} {role}");
                    }
                }
            }
        }
    }

    /// Text on derived chrome keeps the semantic pairs' WCAG AA floor
    /// (4.5:1) wherever text sits: the bar, cards, fields, the menu
    /// highlight, primary buttons, accent fills and Close.
    #[test]
    fn derived_chrome_text_meets_aa() {
        use crate::{Contrast, DesignContext, Mode, Scheme, contrast_ratio};
        let document = embedded();
        let mut failures = Vec::new();
        for scheme in Scheme::REVISION_ONE {
            for mode in Mode::ALL {
                for contrast in Contrast::ALL {
                    let context = DesignContext {
                        scheme,
                        mode,
                        contrast,
                        ..DesignContext::default()
                    };
                    let chrome = compiled(&document, context)
                        .dictionary()
                        .chrome
                        .clone()
                        .unwrap();
                    for (text, surface) in [
                        ("text", "chrome"),
                        ("text", "card"),
                        ("text", "field"),
                        ("text", "hover"),
                        ("menu_highlight_text", "menu_highlight"),
                        ("primary_text", "primary_bg"),
                        ("accent_text", "accent"),
                        ("text", "accent"),
                        ("text", "accent_soft"),
                    ]
                    .into_iter()
                    // Every text level on every surface text is drawn on.
                    .chain(
                        ["text", "icon"]
                            .iter()
                            .chain(FLOORED.iter())
                            .flat_map(|text| BACKGROUNDS.iter().map(move |bg| (*text, *bg))),
                    ) {
                        let ratio =
                            // As drawn: both colours quantised to 8-bit sRGB.
                            contrast_ratio(
                                chrome.get(text).unwrap().rendered(),
                                chrome.get(surface).unwrap().rendered(),
                            );
                        if ratio < 4.5 {
                            failures.push(format!(
                                "{scheme:?}/{mode:?}/{contrast:?} {text} on {surface}: {ratio:.2}"
                            ));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A chrome colour a scheme's block authors wins over the derivation;
    /// the rest of that scheme's chrome still derives.
    #[test]
    fn an_authored_chrome_colour_wins_over_the_derivation() {
        use crate::{DesignContext, Mode, Scheme};
        let mut document = embedded();
        let red = crate::OklchSource {
            color_space: crate::ColourSpace::Oklch,
            l: 0.6,
            c: 0.2,
            h: 25.0,
            alpha: 1.0,
        };
        let block = document
            .v1
            .modifiers
            .iter_mut()
            .find(|b| {
                b.when.get(&crate::ModifierAxis::Scheme).map(String::as_str) == Some("forest")
                    && b.when.get(&crate::ModifierAxis::Mode).map(String::as_str) == Some("light")
            })
            .expect("a forest light block");
        block.primitives.colors.insert("chrome.accent".into(), red);
        let forest = DesignContext {
            scheme: Scheme::Forest,
            mode: Mode::Light,
            ..DesignContext::default()
        };
        let design = compiled(&document, forest);
        let chrome = design.dictionary().chrome.clone().unwrap();
        let primitives = &design.dictionary().colours.primitives;
        assert_eq!(
            chrome.get("accent"),
            Some(primitives["chrome.accent"]),
            "the authored accent"
        );
        assert_eq!(
            chrome.get("chrome"),
            Some(design.dictionary().colours.pairs["base"].rendered_surface),
            "the rest still derives"
        );
    }

    #[test]
    fn a_bad_derivation_is_an_error_in_every_context() {
        let mut s = source(
            CoveragePolicy::Explicit,
            &[("panel", "p.panel"), ("text", "p.text")],
        );
        s.derive.insert(
            "panel".into(),
            ChromeDeriveSource::Pair {
                pair: "nope".into(),
                part: PairPart::Surface,
            },
        );
        s.derive.insert(
            "bogus".into(),
            ChromeDeriveSource::NonText {
                value: "border".into(),
            },
        );
        let codes: Vec<_> = run(Some(&s), &["p.panel", "p.text"])
            .unwrap_err()
            .iter()
            .map(|d| d.code)
            .collect();
        assert!(
            codes.contains(&"invalid-chrome-derivation") && codes.contains(&"unknown-chrome-role"),
            "{codes:?}"
        );
    }

    #[test]
    fn a_derivation_outranks_a_base_declaration_but_not_a_block() {
        let colours = ResolvedColours {
            primitives: primitives(),
            ..Default::default()
        };
        let mut s = source(
            CoveragePolicy::Explicit,
            &[("panel", "p.panel"), ("text", "p.text")],
        );
        s.derive.insert(
            "panel".into(),
            ChromeDeriveSource::Primitive {
                value: "p.text".into(),
            },
        );
        let base = BTreeSet::new();
        let chrome = compile_roles(
            Some(&s),
            &Inputs {
                colours: &colours,
                authored: &base,
            },
            ROLES,
        )
        .unwrap()
        .0
        .unwrap();
        assert_eq!(
            chrome.get("panel"),
            Some(LinearRgba::WHITE),
            "derived over the base declaration"
        );
        let block = BTreeSet::from(["p.panel".to_owned()]);
        let chrome = compile_roles(
            Some(&s),
            &Inputs {
                colours: &colours,
                authored: &block,
            },
            ROLES,
        )
        .unwrap()
        .0
        .unwrap();
        assert_eq!(
            chrome.get("panel"),
            Some(LinearRgba::BLACK),
            "the block's own colour"
        );
    }
}
