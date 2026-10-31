// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Snapshot schema 2: the wire form with each distinct design projection
//! sent once.
//!
//! A schema 1 [`Snapshot`] carries a full design projection in every
//! effective context, though the contexts usually share one. Schema 2 sends
//! each distinct projection once under `designs`, keyed by its digest, and
//! each context names its digest. It is only a wire form: [`decode`] expands
//! it back into the same in-memory [`Snapshot`] a schema 1 read gives, so
//! the consumer, reducer, cache and effective digests are unchanged.
//!
//! A digest is an opaque key here: the decoder requires every context's
//! digest to name a sent projection and every sent projection to be named,
//! and does not recompute digests (key order in a re-serialised projection
//! is not part of the contract).
use crate::model::{
    Binding, CommonUi, Desktop, Effective, Revision, Snapshot, default_caption_side,
    default_decorations, is_default_caption_side, is_default_decorations,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The schema of [`CompactSnapshot`].
pub const COMPACT_SCHEMA: u32 = 2;

/// How much larger than its schema 1 snapshot a compact one may be. When
/// every context has a design of its own nothing is shared, and each design
/// costs a digest key and a digest reference (about 140 bytes); a profile
/// has at most the desktop, the native apps and 16 app overrides. A compact
/// body may exceed [`crate::MAX_SNAPSHOT_BYTES`] by this much; the expanded
/// snapshot never may.
pub const MAX_OVERHEAD_BYTES: usize = 8 * 1024;

/// The retained topic carrying the profile's snapshot in schema 2; owned by
/// settingsd like [`crate::topic`], which carries schema 1.
pub const COMPACT_TOPIC_PREFIX: &str = "settingsd.desktop.compact.";

pub fn compact_topic(profile: &str) -> String {
    format!("{COMPACT_TOPIC_PREFIX}{profile}")
}

/// A [`Snapshot`] in schema 2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactSnapshot {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    pub source_digest: String,
    pub desktop: Desktop,
    pub effective: BTreeMap<String, CompactEffective>,
    /// Each distinct projection once, by digest.
    pub designs: BTreeMap<String, design::DesignReadProjection>,
}

/// An [`Effective`] whose `design` names a projection in
/// [`CompactSnapshot::designs`]. Its other fields serialise exactly as
/// [`Effective`]'s do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactEffective {
    pub scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    pub mode: String,
    pub contrast: String,
    #[serde(
        default = "default_decorations",
        skip_serializing_if = "is_default_decorations"
    )]
    pub decorations: String,
    #[serde(
        default = "default_caption_side",
        skip_serializing_if = "is_default_caption_side"
    )]
    pub caption_side: String,
    #[serde(
        default = "crate::model::default_window_style",
        skip_serializing_if = "crate::model::is_default_window_style"
    )]
    pub window_style: String,
    #[serde(
        default = "crate::model::default_menu_style",
        skip_serializing_if = "crate::model::is_default_menu_style"
    )]
    pub menu_style: String,
    #[serde(
        default = "crate::model::default_menu_side",
        skip_serializing_if = "crate::model::is_default_menu_side"
    )]
    pub menu_side: String,
    #[serde(
        default = "crate::model::default_actions_place",
        skip_serializing_if = "crate::model::is_default_actions_place"
    )]
    pub actions_place: String,
    #[serde(
        default = "crate::model::default_actions_align",
        skip_serializing_if = "crate::model::is_default_actions_align"
    )]
    pub actions_align: String,
    #[serde(
        default = "crate::model::default_title_align",
        skip_serializing_if = "crate::model::is_default_title_align"
    )]
    pub title_align: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_stroke: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_family: Option<String>,
    // Key space v2's sealed look inputs ride schema 2 without a bump: they
    // are omitted wherever the format-4 path has none, and only v2 effective
    // maps (not served before S6) carry them. Every reader of this shape
    // (Quoin's consumer, compd decor, the cache) moves in the same gold
    // install as the writer (group B); S7's schema 3 replaces this shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<design::SrgbColour>,
    #[serde(
        default = "crate::model::default_roundness",
        skip_serializing_if = "crate::model::is_default_roundness"
    )]
    pub roundness: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tune: BTreeMap<String, serde_json::Value>,
    pub ui: CommonUi,
    /// The digest of this context's projection.
    pub design: String,
    pub provenance: BTreeMap<String, String>,
}

/// `snapshot` in schema 2.
pub fn encode(snapshot: &Snapshot) -> Result<CompactSnapshot, serde_json::Error> {
    let mut designs = BTreeMap::new();
    let mut effective = BTreeMap::new();
    for (context, e) in &snapshot.effective {
        let digest = crate::digest(&e.design)?;
        designs
            .entry(digest.clone())
            .or_insert_with(|| e.design.clone());
        effective.insert(
            context.clone(),
            CompactEffective {
                scheme: e.scheme.clone(),
                style: e.style.clone(),
                mode: e.mode.clone(),
                contrast: e.contrast.clone(),
                decorations: e.decorations.clone(),
                caption_side: e.caption_side.clone(),
                actions_place: e.actions_place.clone(),
                actions_align: e.actions_align.clone(),
                title_align: e.title_align.clone(),
                window_style: e.window_style.clone(),
                menu_style: e.menu_style.clone(),
                menu_side: e.menu_side.clone(),
                icon_stroke: e.icon_stroke,
                font_family: e.font_family.clone(),
                font_weight: e.font_weight,
                font_size: e.font_size,
                mono_family: e.mono_family.clone(),
                accent: e.accent,
                roundness: e.roundness.clone(),
                tune: e.tune.clone(),
                ui: e.ui.clone(),
                design: digest,
                provenance: e.provenance.clone(),
            },
        );
    }
    Ok(CompactSnapshot {
        schema: COMPACT_SCHEMA,
        binding: snapshot.binding.clone(),
        incarnation: snapshot.incarnation.clone(),
        revision: snapshot.revision,
        design_revision: snapshot.design_revision,
        source_digest: snapshot.source_digest.clone(),
        desktop: snapshot.desktop.clone(),
        effective,
        designs,
    })
}

impl CompactSnapshot {
    /// The schema 1 [`Snapshot`] this encodes.
    pub fn expand(self) -> Result<Snapshot, String> {
        if self.schema != COMPACT_SCHEMA {
            return Err(format!("schema {} is not {COMPACT_SCHEMA}", self.schema));
        }
        let mut unused: std::collections::BTreeSet<&String> = self.designs.keys().collect();
        let mut effective = BTreeMap::new();
        for (context, e) in self.effective {
            let Some(design) = self.designs.get(&e.design) else {
                return Err(format!("{context} names an unknown design {}", e.design));
            };
            unused.remove(&e.design);
            effective.insert(
                context,
                Effective {
                    scheme: e.scheme,
                    style: e.style,
                    mode: e.mode,
                    contrast: e.contrast,
                    decorations: e.decorations,
                    caption_side: e.caption_side,
                    actions_place: e.actions_place,
                    actions_align: e.actions_align,
                    title_align: e.title_align,
                    window_style: e.window_style,
                    menu_style: e.menu_style,
                    menu_side: e.menu_side,
                    icon_stroke: e.icon_stroke,
                    font_family: e.font_family,
                    font_weight: e.font_weight,
                    font_size: e.font_size,
                    mono_family: e.mono_family,
                    accent: e.accent,
                    roundness: e.roundness,
                    tune: e.tune,
                    ui: e.ui,
                    design: design.clone(),
                    provenance: e.provenance,
                },
            );
        }
        if let Some(digest) = unused.first() {
            return Err(format!("design {digest} is not named by any context"));
        }
        Ok(Snapshot {
            schema: crate::SCHEMA,
            binding: self.binding,
            incarnation: self.incarnation,
            revision: self.revision,
            design_revision: self.design_revision,
            source_digest: self.source_digest,
            desktop: self.desktop,
            effective,
        })
    }
}

/// A snapshot in schema 1 or 2, as the in-memory [`Snapshot`].
pub fn decode(value: serde_json::Value) -> Result<Snapshot, String> {
    match value.get("schema").and_then(serde_json::Value::as_u64) {
        Some(2) => serde_json::from_value::<CompactSnapshot>(value)
            .map_err(|e| e.to_string())?
            .expand(),
        _ => serde_json::from_value::<Snapshot>(value).map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(desktop: Desktop) -> Snapshot {
        let effective = crate::resolve(&desktop).unwrap();
        Snapshot {
            schema: crate::SCHEMA,
            binding: Binding {
                instance: "example".into(),
                profile: "default".into(),
            },
            incarnation: "inc".into(),
            revision: Revision(3),
            design_revision: Revision(2),
            source_digest: "d".into(),
            desktop,
            effective,
        }
    }

    #[test]
    fn one_projection_for_contexts_that_share_it_and_an_exact_round_trip() {
        let mut desktop = Desktop::default();
        desktop.apps.insert(
            "term".into(),
            crate::AppOverride {
                scheme: Some("ocean".into()),
                ..Default::default()
            },
        );
        let full = snapshot(desktop);
        let compact = encode(&full).unwrap();
        assert_eq!(compact.designs.len(), 2, "the desktop's and term's");
        let wire = serde_json::to_value(&compact).unwrap();
        assert_eq!(wire["schema"], 2);
        let back = decode(wire.clone()).unwrap();
        assert_eq!(back, full);
        assert_eq!(
            crate::digest(&back.effective).unwrap(),
            crate::digest(&full.effective).unwrap()
        );
        let full_len = serde_json::to_vec(&full).unwrap().len();
        let compact_len = serde_json::to_vec(&wire).unwrap().len();
        assert!(compact_len * 3 < full_len, "{compact_len} vs {full_len}");
    }

    /// Nothing shared is the compact form's worst case: it must stay within
    /// [`MAX_OVERHEAD_BYTES`] of schema 1 with every context distinct.
    #[test]
    fn with_no_design_shared_the_overhead_stays_bounded() {
        let mut desktop = Desktop::default();
        let schemes = [
            "ocean", "crimson", "stone", "forest", "sunset", "mono", "pro", "studio", "classic",
            "breeze", "adwaita",
        ];
        for i in 0..16 {
            desktop.apps.insert(
                format!("app{i:02}"),
                crate::AppOverride {
                    scheme: Some(schemes[i % schemes.len()].into()),
                    mode: Some(if i < schemes.len() { "light" } else { "dark" }.into()),
                    ..Default::default()
                },
            );
        }
        let full = snapshot(desktop);
        let compact = encode(&full).unwrap();
        let full_len = serde_json::to_vec(&full).unwrap().len();
        let compact_len = serde_json::to_vec(&compact).unwrap().len();
        assert!(compact.designs.len() >= 17, "{}", compact.designs.len());
        assert!(
            compact_len <= full_len + MAX_OVERHEAD_BYTES,
            "{compact_len} vs {full_len}"
        );
    }

    #[test]
    fn schema_1_still_decodes() {
        let full = snapshot(Desktop::default());
        assert_eq!(decode(serde_json::to_value(&full).unwrap()).unwrap(), full);
    }

    #[test]
    fn window_style_round_trips_in_both_schemas_and_legacy_defaults_are_omitted() {
        let old = snapshot(Desktop::default());
        for value in [
            serde_json::to_value(&old).unwrap(),
            serde_json::to_value(encode(&old).unwrap()).unwrap(),
        ] {
            assert!(value["desktop"]["appearance"].get("window_style").is_none());
            assert!(value["effective"]["desktop"].get("window_style").is_none());
            assert_eq!(
                decode(value).unwrap().effective["desktop"].window_style,
                "mac"
            );
        }
        for name in design::WindowStyle::NAMES
            .iter()
            .copied()
            .chain(["future-style"])
        {
            let mut desktop = Desktop::default();
            desktop.appearance.window_style = name.into();
            let full = snapshot(desktop);
            assert_eq!(decode(serde_json::to_value(&full).unwrap()).unwrap(), full);
            assert_eq!(
                decode(serde_json::to_value(encode(&full).unwrap()).unwrap()).unwrap(),
                full
            );
        }
    }

    #[test]
    fn dangling_and_unnamed_designs_are_refused() {
        let full = snapshot(Desktop::default());
        let mut wire = serde_json::to_value(encode(&full).unwrap()).unwrap();
        let digest = wire["effective"]["desktop"]["design"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut dangling = wire.clone();
        dangling["effective"]["desktop"]["design"] = json!("missing");
        assert!(decode(dangling).unwrap_err().contains("unknown design"));
        wire["designs"]["extra"] = wire["designs"][&digest].clone();
        assert!(decode(wire).unwrap_err().contains("not named"));
    }

    #[test]
    fn menus_round_trip_and_legacy_defaults_are_omitted() {
        let old = snapshot(Desktop::default());
        for value in [
            serde_json::to_value(&old).unwrap(),
            serde_json::to_value(encode(&old).unwrap()).unwrap(),
        ] {
            for key in [
                "menu_style",
                "menu_side",
                "title_align",
                "actions_align",
                "actions_place",
                "caption_side",
            ] {
                assert!(value["desktop"]["appearance"].get(key).is_none());
                assert!(value["effective"]["desktop"].get(key).is_none());
            }
            let decoded = decode(value).unwrap();
            assert_eq!(decoded.effective["desktop"].menu_style, "bar");
            assert_eq!(decoded.effective["desktop"].menu_side, "left");
        }
        for style in design::MenuStyle::NAMES
            .iter()
            .copied()
            .chain(["future-style"])
        {
            for side in design::MenuSide::NAMES
                .iter()
                .copied()
                .chain(["future-side"])
            {
                let mut desktop = Desktop::default();
                desktop.appearance.menu_style = style.into();
                desktop.appearance.menu_side = side.into();
                let full = snapshot(desktop);
                assert_eq!(decode(serde_json::to_value(&full).unwrap()).unwrap(), full);
                assert_eq!(
                    decode(serde_json::to_value(encode(&full).unwrap()).unwrap()).unwrap(),
                    full
                );
            }
        }
    }

    #[test]
    fn title_band_names_survive_full_and_compact_round_trips() {
        for (axis, names) in [
            ("title_align", design::TitleAlign::NAMES),
            ("actions_align", design::ActionsAlign::NAMES),
            ("actions_place", design::ActionsPlace::NAMES),
            ("caption_side", design::CaptionSide::NAMES),
        ] {
            for name in names.iter().copied().chain(["future-choice"]) {
                let desktop = crate::v1::patch(
                    &Desktop::default(),
                    &std::collections::BTreeMap::from([(
                        format!("appearance.{axis}"),
                        json!(name),
                    )]),
                    &[],
                )
                .unwrap();
                let full = snapshot(desktop);
                assert_eq!(decode(serde_json::to_value(&full).unwrap()).unwrap(), full);
                assert_eq!(
                    decode(serde_json::to_value(encode(&full).unwrap()).unwrap()).unwrap(),
                    full,
                );
                let saved = serde_json::to_value(&full.desktop.appearance).unwrap();
                let default = match axis {
                    "actions_align" => "right",
                    "actions_place" => "row",
                    _ => "auto",
                };
                assert_eq!(
                    saved
                        .get(axis)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(default),
                    name,
                );
            }
        }
    }
}
