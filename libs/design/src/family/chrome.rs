//! The chrome family: the exact colours of application chrome (title bars,
//! menus, tabs, panels, controls) as one closed set of roles, each naming a
//! colour primitive.
//!
//! No contrast check, recipe or derivation applies. A role renders exactly as
//! its primitive is authored; written as OKLCH at nine decimals, every 8-bit
//! sRGB value round-trips (see `colour_model::exact_authoring_tests`). Schemes
//! and modes change the colours by overlaying the primitives, so the role map
//! itself is authored once.
//!
//! Source shape, under `design.v1.families`:
//!
//! ```text
//! chrome: { coverage: "explicit", roles: { "panel": "chrome.panel", ... } }
//! ```
//!
//! Coverage `explicit` makes a missing role an error; the default `warn`
//! reports it and leaves the role out. An unknown role or primitive is always
//! an error.

use crate::source::{ChromeMappingSource, CoveragePolicy, DesignV1Source};
use crate::{DesignDiagnostic, LinearRgba};
use std::collections::BTreeMap;

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

/// Compile the chrome family against the resolved primitives. `None` when the
/// design authors no chrome family. Errors are fatal; warnings ride along.
pub(crate) fn compile(
    source: &DesignV1Source,
    primitives: &BTreeMap<String, LinearRgba>,
) -> Result<(Option<ResolvedChrome>, Vec<DesignDiagnostic>), Vec<DesignDiagnostic>> {
    compile_roles(source.families.chrome.as_ref(), primitives, ROLES)
}

fn compile_roles(
    family: Option<&ChromeMappingSource>,
    primitives: &BTreeMap<String, LinearRgba>,
    roles: &[&str],
) -> Result<(Option<ResolvedChrome>, Vec<DesignDiagnostic>), Vec<DesignDiagnostic>> {
    let Some(family) = family else {
        return Ok((None, Vec::new()));
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut colours = BTreeMap::new();
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
        match primitives.get(primitive) {
            Some(colour) => {
                colours.insert(role.clone(), *colour);
            }
            None => errors.push(DesignDiagnostic::error(
                "unknown-primitive",
                path,
                format!("`{primitive}` is not a colour primitive"),
            )),
        }
    }
    for role in roles
        .iter()
        .filter(|role| !family.roles.contains_key(**role))
    {
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
        }
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
        let (chrome, warnings) = compile_roles(Some(&s), &primitives(), ROLES).unwrap();
        let chrome = chrome.unwrap();
        assert_eq!(chrome.get("panel"), Some(LinearRgba::BLACK));
        assert_eq!(chrome.get("text"), Some(LinearRgba::WHITE));
        assert!(warnings.is_empty());
    }

    #[test]
    fn explicit_coverage_refuses_a_missing_role_and_warn_reports_it() {
        let explicit = source(CoveragePolicy::Explicit, &[("panel", "p.panel")]);
        let errors = compile_roles(Some(&explicit), &primitives(), ROLES).unwrap_err();
        assert_eq!(errors[0].code, "missing-chrome-role");
        let warn = source(CoveragePolicy::Warn, &[("panel", "p.panel")]);
        let (chrome, warnings) = compile_roles(Some(&warn), &primitives(), ROLES).unwrap();
        assert_eq!(warnings[0].code, "missing-chrome-role");
        assert_eq!(chrome.unwrap().get("text"), None);
    }

    #[test]
    fn unknown_roles_and_primitives_are_errors() {
        let s = source(
            CoveragePolicy::Warn,
            &[("panel", "nope"), ("bogus", "p.text"), ("text", "p.text")],
        );
        let codes: Vec<_> = compile_roles(Some(&s), &primitives(), ROLES)
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
        assert_eq!(compile_roles(None, &primitives(), ROLES).unwrap().0, None);
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
}
