// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Headless appearance projection: the one portal-facing view of a settings
//! snapshot. Pure data and validation; no transport, no renderer.
use crate::{Binding, Diagnostic, Revision, Snapshot};
use serde::{Deserialize, Serialize};

pub const LEGACY_APPEARANCE_SCHEMA: u32 = 1;
pub const APPEARANCE_SCHEMA: u32 = 2;
/// The effective context whose values the appearance projection publishes.
pub const APPEARANCE_CONTEXT: &str = "desktop";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceProjection {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    /// `dark` or `light`, the settings contract's `appearance.mode` enum.
    pub mode: String,
    /// `normal` or `high`, the settings contract's `appearance.contrast` enum.
    pub contrast: String,
    /// `right` or `left`, from the desktop context's caption placement.
    #[serde(
        default = "crate::default_caption_side",
        skip_serializing_if = "crate::is_default_caption_side"
    )]
    pub caption_side: String,
    /// Accent as sRGB, 0.0..=1.0 per channel, alpha excluded.
    pub accent: [f64; 3],
}

impl AppearanceProjection {
    /// The original wire view. Caption placement remains in its atomic snapshot.
    pub fn legacy(mut self) -> Self {
        self.schema = LEGACY_APPEARANCE_SCHEMA;
        self.caption_side = "right".into();
        self
    }

    /// Project the desktop context of an accepted snapshot. The accent is
    /// supplied by the caller because its source is not yet a settings field.
    pub fn from_snapshot(snapshot: &Snapshot, accent: [f64; 3]) -> Result<Self, Diagnostic> {
        let effective = snapshot.effective.get(APPEARANCE_CONTEXT).ok_or_else(|| {
            Diagnostic::new(
                "missing_context",
                "effective",
                "Snapshot has no desktop context",
            )
        })?;
        let projection = Self {
            schema: APPEARANCE_SCHEMA,
            binding: snapshot.binding.clone(),
            incarnation: snapshot.incarnation.clone(),
            revision: snapshot.revision,
            design_revision: snapshot.design_revision,
            mode: effective.mode.clone(),
            contrast: effective.contrast.clone(),
            caption_side: effective.caption_side.clone(),
            accent,
        };
        projection.validate()?;
        Ok(projection)
    }

    /// Refuse anything a consumer must not publish. Callers validate every
    /// projection they receive, not only ones they built.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if !matches!(self.schema, LEGACY_APPEARANCE_SCHEMA | APPEARANCE_SCHEMA) {
            return Err(Diagnostic::new(
                "unsupported_schema",
                "schema",
                format!(
                    "Expected appearance schema {LEGACY_APPEARANCE_SCHEMA} or {APPEARANCE_SCHEMA}"
                ),
            ));
        }
        self.binding.validate()?;
        if !matches!(self.mode.as_str(), "dark" | "light") {
            return Err(Diagnostic::new(
                "invalid_appearance",
                "mode",
                "Use dark or light",
            ));
        }
        if !matches!(self.contrast.as_str(), "normal" | "high") {
            return Err(Diagnostic::new(
                "invalid_appearance",
                "contrast",
                "Use normal or high",
            ));
        }
        if design::CaptionSide::from_name(&self.caption_side).is_none() {
            return Err(Diagnostic::new(
                "invalid_appearance",
                "caption_side",
                "Use right or left",
            ));
        }
        if self.schema == LEGACY_APPEARANCE_SCHEMA && self.caption_side != "right" {
            return Err(Diagnostic::new(
                "invalid_appearance",
                "caption_side",
                "Caption placement requires appearance schema 2",
            ));
        }
        for (index, channel) in self.accent.iter().enumerate() {
            if !channel.is_finite() || !(0.0..=1.0).contains(channel) {
                return Err(Diagnostic::new(
                    "invalid_appearance",
                    &format!("accent[{index}]"),
                    "Channel must be finite and within 0.0..=1.0",
                ));
            }
        }
        Ok(())
    }

    /// Identity of the settings state this projection was built from. Two
    /// projections with equal identity describe the same accepted state.
    pub fn identity(&self) -> (&str, Revision, Revision) {
        (&self.incarnation, self.revision, self.design_revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Desktop, resolve};

    fn snapshot() -> Snapshot {
        let desktop = Desktop::default();
        let effective = resolve(&desktop).unwrap();
        Snapshot {
            schema: crate::SCHEMA,
            binding: Binding {
                instance: "fixture".into(),
                profile: "default".into(),
            },
            incarnation: "inc-1".into(),
            revision: Revision(7),
            design_revision: Revision(3),
            source_digest: String::new(),
            desktop,
            effective,
        }
    }

    #[test]
    fn projects_the_desktop_context_with_its_identity() {
        let projection = AppearanceProjection::from_snapshot(&snapshot(), [0.1, 0.2, 0.3]).unwrap();
        assert_eq!(projection.mode, "dark");
        assert_eq!(projection.contrast, "normal");
        assert_eq!(projection.caption_side, "right");
        assert_eq!(projection.identity(), ("inc-1", Revision(7), Revision(3)));
    }

    #[test]
    fn refuses_out_of_contract_values() {
        let good = AppearanceProjection::from_snapshot(&snapshot(), [0.0, 0.0, 0.0]).unwrap();
        let mut bad = good.clone();
        bad.mode = "sepia".into();
        assert_eq!(bad.validate().unwrap_err().path, "mode");
        let mut bad = good.clone();
        bad.contrast = "extreme".into();
        assert_eq!(bad.validate().unwrap_err().path, "contrast");
        let mut bad = good.clone();
        bad.accent[1] = 1.5;
        assert_eq!(bad.validate().unwrap_err().path, "accent[1]");
        let mut bad = good.clone();
        bad.accent[2] = f64::NAN;
        assert!(bad.validate().is_err());
        let mut bad = good;
        bad.schema = 3;
        assert_eq!(bad.validate().unwrap_err().code, "unsupported_schema");
    }

    #[test]
    fn missing_desktop_context_is_a_diagnostic() {
        let mut snap = snapshot();
        snap.effective.clear();
        let error = AppearanceProjection::from_snapshot(&snap, [0.0; 3]).unwrap_err();
        assert_eq!(error.code, "missing_context");
    }

    #[test]
    fn projects_caption_side_and_reads_old_projections_as_right() {
        let mut snap = snapshot();
        snap.effective
            .get_mut(APPEARANCE_CONTEXT)
            .unwrap()
            .caption_side = "left".into();
        let left = AppearanceProjection::from_snapshot(&snap, [0.0; 3]).unwrap();
        assert_eq!(left.caption_side, "left");
        let encoded = serde_json::to_value(&left).unwrap();
        assert_eq!(encoded["caption_side"], "left");
        assert_eq!(
            serde_json::from_value::<AppearanceProjection>(encoded).unwrap(),
            left
        );

        let right = AppearanceProjection::from_snapshot(&snapshot(), [0.0; 3]).unwrap();
        let right = right.legacy();
        let old = serde_json::to_value(&right).unwrap();
        assert!(old.get("caption_side").is_none());
        assert_eq!(
            serde_json::from_value::<AppearanceProjection>(old).unwrap(),
            right
        );

        let mut bad = left;
        bad.caption_side = "top".into();
        assert_eq!(bad.validate().unwrap_err().path, "caption_side");
    }

    #[test]
    fn legacy_wire_view_omits_caption_side_even_when_left() {
        let mut snap = snapshot();
        snap.effective
            .get_mut(APPEARANCE_CONTEXT)
            .unwrap()
            .caption_side = "left".into();
        let projection = AppearanceProjection::from_snapshot(&snap, [0.0; 3]).unwrap();
        assert_eq!(projection.schema, APPEARANCE_SCHEMA);
        let legacy = projection.legacy();
        legacy.validate().unwrap();
        let wire = serde_json::to_value(&legacy).unwrap();
        assert_eq!(wire["schema"], LEGACY_APPEARANCE_SCHEMA);
        assert!(wire.get("caption_side").is_none());
    }
}
