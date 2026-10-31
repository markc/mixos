// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One render-domain comparison for all native consumers. Authority identity,
//! source names, revisions and provenance are evidence, not render inputs.
use crate::registry::{self, Domain, Values};
use crate::{CommonUi, Effective, Snapshot};
use design::{DesignReadProjection, ReadButton, ReadPair, ReadType};
use std::collections::BTreeSet;

const DOMAINS: [Domain; 9] = [
    Domain::Paint,
    Domain::Text,
    Domain::Layout,
    Domain::Resources,
    Domain::Motion,
    Domain::Shell,
    Domain::Input,
    Domain::Export,
    Domain::System,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangePlan {
    pub paint: bool,
    pub text: bool,
    pub layout: bool,
    pub resources: bool,
    pub motion: bool,
    pub shell: bool,
    /// Bindings, pointer and keyboard behaviour (key space v2).
    pub input: bool,
    /// Read only by the Mix exporters.
    pub export: bool,
    /// Applied to the machine by an owner (monitord, hostd, xdg-export…).
    pub system: bool,
}
impl ChangePlan {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    fn mark(&mut self, domain: Domain) {
        let flag = match domain {
            Domain::Paint => &mut self.paint,
            Domain::Text => &mut self.text,
            Domain::Layout => &mut self.layout,
            Domain::Resources => &mut self.resources,
            Domain::Motion => &mut self.motion,
            Domain::Shell => &mut self.shell,
            Domain::Input => &mut self.input,
            Domain::Export => &mut self.export,
            Domain::System => &mut self.system,
        };
        *flag = true;
    }

    /// The plan for a change of authored values (key space v2), from each
    /// changed key's registry domains (`KeyDef.domains`; a `tune.style`
    /// token's own, from `design::tunables()`). A key changes when its
    /// authored-or-default value differs. It may over-report (a mode flip
    /// from `dark` to `auto` that still resolves dark is a paint change),
    /// never under-report. As [`Self::between`] has it, fresh resources
    /// imply re-shaped text, text and the shell imply layout.
    pub fn of_values(old: &Values, new: &Values) -> Self {
        let mut plan = Self::default();
        let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        for path in keys {
            if old.get(path) == new.get(path) {
                continue;
            }
            let Ok(m) = registry::lookup(path) else {
                // Not a key: validation refuses it before any plan is made.
                continue;
            };
            if let Some(token) = path.strip_prefix("tune.")
                && let Some(tunable) = registry::tunable(token)
            {
                for name in &tunable.domains {
                    if let Some(domain) = DOMAINS.iter().find(|d| d.name() == *name) {
                        plan.mark(*domain);
                    }
                }
            }
            for domain in m.def.domains {
                plan.mark(*domain);
            }
        }
        plan.text |= plan.resources;
        plan.layout |= plan.text || plan.shell;
        plan
    }
    /// Resource preparation completes before activating any of these domains.
    pub fn between(
        previous: Option<&Snapshot>,
        next: &Snapshot,
        context: &str,
        shell: bool,
    ) -> Self {
        let old = previous.and_then(|s| s.effective.get(context));
        let new = next.effective.get(context);
        let mut plan = match (old, new) {
            (Some(old), Some(new)) => compare(old, new),
            _ => Self {
                paint: true,
                text: true,
                layout: true,
                resources: true,
                motion: true,
                shell: false,
                ..Self::default()
            },
        };
        plan.shell = shell && previous.is_none_or(|s| s.desktop.shell != next.desktop.shell);
        plan.layout |= plan.shell;
        plan
    }
}
fn pair_key(pair: &ReadPair) -> impl PartialEq + '_ {
    let ReadPair {
        surface,
        foreground,
        rendered_surface,
        rendered_foreground,
        backdrop,
        contrast_ratio: _,
    } = pair;
    (
        surface,
        foreground,
        rendered_surface,
        rendered_foreground,
        backdrop,
    )
}
fn button_key(button: &ReadButton) -> (&str, &str, &str, bool) {
    // The remaining fields are mapped by paint/text/layout below. Exhaustive
    // destructuring forces every added DTO field to be classified deliberately.
    let ReadButton {
        variant,
        size,
        interaction,
        focus_visible,
        pair: _,
        border: _,
        ring: _,
        height: _,
        min_width: _,
        padding_x: _,
        border_width: _,
        radius: _,
        typography: _,
    } = button;
    (variant, size, interaction, *focus_visible)
}
fn font_key(record: &ReadType) -> impl PartialEq + '_ {
    let ReadType {
        family,
        fallbacks,
        generic,
        weight,
        font_size: _,
        line_height: _,
    } = record;
    (family, fallbacks, generic, weight)
}
fn view(effective: &Effective) -> &DesignReadProjection {
    let Effective {
        scheme: _,
        style: _,
        mode: _,
        contrast: _,
        decorations: _,
        caption_side: _,
        window_style: _,
        menu_style: _,
        menu_side: _,
        actions_place: _,
        actions_align: _,
        title_align: _,
        // Their classes are in `compare`: stroke paints, fonts reshape text.
        icon_stroke: _,
        font_family: _,
        font_weight: _,
        font_size: _,
        mono_family: _,
        ui,
        design,
        provenance: _,
    } = effective;
    let CommonUi {
        density: _,
        text_scale: _,
        reduced_motion: _,
    } = ui;
    let DesignReadProjection {
        schema: _,
        source: _,
        primitives: _,
        pairs: _,
        non_text: _,
        metrics: _,
        scales: _,
        typography: _,
        buttons: _,
    } = design;
    // Source/provenance/schema identify and validate evidence; all remaining
    // fields feed the domains below. No wildcard permits an unnoticed addition.
    design
}
fn compare(old: &Effective, new: &Effective) -> ChangePlan {
    let a = view(old);
    let b = view(new);
    // The session font choices change the faces the text is drawn with.
    let fonts = old.font_family != new.font_family
        || old.font_weight != new.font_weight
        || old.font_size != new.font_size
        || old.mono_family != new.mono_family;
    let resources = fonts
        || !a
            .typography
            .iter()
            .map(|(k, r)| (k, font_key(r)))
            .eq(b.typography.iter().map(|(k, r)| (k, font_key(r))));
    let text = resources
        || old.ui.text_scale != new.ui.text_scale
        || !a
            .typography
            .iter()
            .map(|(k, r)| (k, r.font_size, r.line_height))
            .eq(b
                .typography
                .iter()
                .map(|(k, r)| (k, r.font_size, r.line_height)))
        || !a
            .buttons
            .iter()
            .map(|c| (button_key(c), &c.typography))
            .eq(b.buttons.iter().map(|c| (button_key(c), &c.typography)));
    // A style changes the chrome's forms and lengths; the framing, the
    // title bar's layout.
    let style = old.style != new.style;
    let window_style = design::WindowStyle::resolve(&old.window_style)
        != design::WindowStyle::resolve(&new.window_style);
    let band_title_align = design::TitleAlign::resolve(&old.title_align)
        != design::TitleAlign::resolve(&new.title_align);
    let band_actions_align = design::ActionsAlign::resolve(&old.actions_align)
        != design::ActionsAlign::resolve(&new.actions_align);
    let band_actions_place = design::ActionsPlace::resolve(&old.actions_place)
        != design::ActionsPlace::resolve(&new.actions_place);
    let menus = design::MenuStyle::resolve(&old.menu_style)
        != design::MenuStyle::resolve(&new.menu_style)
        || design::MenuSide::resolve(&old.menu_side) != design::MenuSide::resolve(&new.menu_side);
    let framing = old.decorations != new.decorations
        || design::CaptionSide::resolve(&old.caption_side)
            != design::CaptionSide::resolve(&new.caption_side)
        || window_style
        || band_title_align
        || band_actions_place
        || band_actions_align
        || menus;
    let paint = band_actions_place
        || band_actions_align
        || band_title_align
        || window_style
        || menus
        || style
        || old.icon_stroke != new.icon_stroke
        || old.scheme != new.scheme
        || old.mode != new.mode
        || old.contrast != new.contrast
        || a.primitives != b.primitives
        || a.non_text != b.non_text
        || !a
            .pairs
            .iter()
            .map(|(k, p)| (k, pair_key(p)))
            .eq(b.pairs.iter().map(|(k, p)| (k, pair_key(p))))
        || !a
            .buttons
            .iter()
            .map(|c| (button_key(c), pair_key(&c.pair), c.border, c.ring))
            .eq(b
                .buttons
                .iter()
                .map(|c| (button_key(c), pair_key(&c.pair), c.border, c.ring)));
    let layout = text
        || style
        || framing
        || old.ui.density != new.ui.density
        || a.metrics != b.metrics
        || a.scales != b.scales
        || !a
            .buttons
            .iter()
            .map(|c| {
                (
                    button_key(c),
                    c.height,
                    c.min_width,
                    c.padding_x,
                    c.border_width,
                    c.radius,
                )
            })
            .eq(b.buttons.iter().map(|c| {
                (
                    button_key(c),
                    c.height,
                    c.min_width,
                    c.padding_x,
                    c.border_width,
                    c.radius,
                )
            }));
    ChangePlan {
        paint,
        text,
        layout,
        resources,
        motion: old.ui.reduced_motion != new.ui.reduced_motion,
        shell: false,
        ..ChangePlan::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_style_invalidates_chrome_paint_and_layout() {
        let resolved = crate::resolve(&crate::Desktop::default()).unwrap();
        let before = &resolved["desktop"];
        let mut after = before.clone();
        after.window_style = "win11".into();
        let plan = compare(before, &after);
        assert!(plan.paint && plan.layout);
        assert!(!plan.resources && !plan.text);
        after.window_style = "future-style".into();
        assert!(compare(before, &after).is_empty());
    }

    fn plan_of(changes: &[(&str, serde_json::Value)]) -> ChangePlan {
        let changes = changes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let next = registry::apply(&Values::new(), &changes, &[]).unwrap();
        ChangePlan::of_values(&Values::new(), &next)
    }

    /// Every stored key's change marks its registry domains, plus the
    /// implications (resources → text → layout, shell → layout), in both
    /// directions (set and reset).
    #[test]
    fn every_key_s_change_marks_its_registry_domains() {
        for def in registry::keys().iter().filter(|d| !d.is_live()) {
            let changes = registry::tests::sample_batch(def).into_iter().collect();
            let next = registry::apply(&Values::new(), &changes, &[]).unwrap();
            for plan in [
                ChangePlan::of_values(&Values::new(), &next),
                ChangePlan::of_values(&next, &Values::new()),
            ] {
                let mut want = ChangePlan::default();
                for domain in def.domains {
                    want.mark(*domain);
                }
                let marked = [
                    (want.paint, plan.paint),
                    (want.text, plan.text),
                    (want.layout, plan.layout),
                    (want.resources, plan.resources),
                    (want.motion, plan.motion),
                    (want.shell, plan.shell),
                    (want.input, plan.input),
                    (want.export, plan.export),
                    (want.system, plan.system),
                ];
                assert!(
                    marked.iter().all(|(want, got)| !want || *got),
                    "{}: {plan:?}",
                    def.path
                );
                assert!(!plan.resources || plan.text, "{}", def.path);
                assert!(!(plan.text || plan.shell) || plan.layout, "{}", def.path);
            }
        }
        assert!(ChangePlan::of_values(&Values::new(), &Values::new()).is_empty());
    }

    /// The change plan per domain, one representative key each.
    #[test]
    fn the_change_plan_per_domain() {
        use serde_json::json;
        let only = |plan: ChangePlan, want: ChangePlan| assert_eq!(plan, want);
        only(
            plan_of(&[("window.clip_tiled", json!(true))]),
            ChangePlan {
                paint: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("window.gaps", json!(8))]),
            ChangePlan {
                layout: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("appearance.font_family", json!("Noto Sans"))]),
            ChangePlan {
                text: true,
                layout: true,
                resources: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("accessibility.reduced_motion", json!(true))]),
            ChangePlan {
                motion: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("time.applet.show_seconds", json!(true))]),
            ChangePlan {
                shell: true,
                layout: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("input.mouse.speed", json!(70))]),
            ChangePlan {
                input: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("appearance.icon_theme", json!("Adwaita"))]),
            ChangePlan {
                export: true,
                ..ChangePlan::default()
            },
        );
        only(
            plan_of(&[("displays.DP-2.scale", json!(150))]),
            ChangePlan {
                system: true,
                ..ChangePlan::default()
            },
        );
        // A style token takes its own domains from the design catalogue.
        let radius = design::tunables()
            .iter()
            .find(|t| t.name == "style.radius")
            .expect("style.radius is a tunable");
        let plan = plan_of(&[(
            "tune.style.radius",
            registry::tests::tunable_sample("tune.style.radius"),
        )]);
        for name in &radius.domains {
            let want = match *name {
                "paint" => plan.paint,
                "layout" => plan.layout,
                "text" => plan.text,
                "resources" => plan.resources,
                "motion" => plan.motion,
                other => panic!("unexpected domain {other}"),
            };
            assert!(want, "style.radius {name}: {plan:?}");
        }
        assert!(!plan.is_empty());
    }

    #[test]
    fn menu_changes_invalidate_paint_and_layout_only() {
        let resolved = crate::resolve(&crate::Desktop::default()).unwrap();
        let before = &resolved["desktop"];
        for (style, side) in [("button", "left"), ("titlebar", "right"), ("bar", "right")] {
            let mut after = before.clone();
            after.menu_style = style.into();
            after.menu_side = side.into();
            let plan = compare(before, &after);
            assert!(plan.paint && plan.layout);
            assert!(!plan.resources && !plan.text);
        }
        let mut future = before.clone();
        future.menu_style = "future-style".into();
        future.menu_side = "future-side".into();
        assert!(compare(before, &future).is_empty());
    }
}
