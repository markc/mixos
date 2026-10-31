// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The frozen format-4 record (formats 1 to 4): read, verified and upgraded
//! to format 4 exactly as the settingsd before key space v2 did, so a
//! format-4 profile migrates (`crate::migration`) and a downgrade writes
//! one a format-4 binary accepts (`crate::downgrade`). Nothing serves it.
//! It goes with [`settings::v1`], one release after every profile migrated.
use serde::{Deserialize, Serialize};
use settings::*;

/// The last format of this record.
///
/// History:
/// - 1: the original record, with no `format` field.
/// - 2: the record seals its format; settings 0.3.5 adds `appearance.style`,
///   `appearance.decorations` and `appearance.caption_side` (authored,
///   omitted at their defaults) and their resolved values in `Effective`.
/// - 3: settings 0.6.0 adds `appearance.icon_stroke`, `appearance.font_family`,
///   `appearance.font_weight` and `appearance.mono_family` (authored and
///   omitted when unset) and their resolved values in `Effective`.
/// - 4: Workshop replaces the retired app context; drop its old authored
///   section and re-derive the effective map through the sealed upgrade path.
///
/// Window style is omitted at mac in both authored and effective values,
/// preserving existing digests without a format change.
pub const FORMAT: u32 = 4;

/// Exact shipped defaults, never a licence to repair arbitrary authored sources.
/// The older installed default (740c0c13) still resolves and is left pinned.
/// v0.1.2 (fe9dfa90) shipped the source last changed by bc2faa69; 5eff67b3
/// retired its panel tokens; current validation also rejects its contrast.
const RETIRED_EMBEDDED_DEFAULTS: &[&str] = &[
    "33a33291c676de5bd1c30cb395534e8656244f57e6771a98bd61e3669e11f235", // 740c0c13
    "34726165cf21927a700a0eb8e5770f6a374d44a7cc2f2fc09d987faf2a288ce2", // bc2faa69 / v0.1.2
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema: u32,
    /// The sealed record's format ([`FORMAT`]); absent in format 1.
    #[serde(default = "format_one", skip_serializing_if = "is_format_one")]
    pub format: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    pub desktop: Desktop,
    pub embedded_source: String,
    pub effective_digest: String,
    pub receipts: Vec<Receipt>,
    pub content_digest: String,
}
fn format_one() -> u32 {
    1
}
fn is_format_one(format: &u32) -> bool {
    *format == 1
}
impl Record {
    /// A first format-4 record, as `settingsd init` wrote one before
    /// format 5: revision 1, no receipts, the shipped default source.
    pub fn initial(binding: Binding, desktop: Desktop) -> anyhow::Result<Self> {
        let effective =
            v1::resolve(&desktop).map_err(|e| anyhow::anyhow!("invalid initial desktop: {e:?}"))?;
        let mut record = Self {
            schema: SCHEMA,
            format: FORMAT,
            binding,
            incarnation: uuid::Uuid::now_v7().to_string(),
            revision: Revision(1),
            design_revision: Revision(1),
            desktop,
            embedded_source: settings::EMBEDDED_DEFAULT_SOURCE.into(),
            effective_digest: settings::digest(&effective)?,
            receipts: Vec::new(),
            content_digest: String::new(),
        };
        record.seal()?;
        record.check(&record.binding)?;
        Ok(record)
    }
    /// The content digest in the record's own format: format 1 sealed the
    /// fields below; later formats also seal the format itself, so a record
    /// cannot be relabelled older without breaking its seal.
    fn digest(&self) -> anyhow::Result<String> {
        let fields = (
            &self.schema,
            &self.binding,
            &self.incarnation,
            &self.revision,
            &self.design_revision,
            &self.desktop,
            &self.embedded_source,
            &self.effective_digest,
            &self.receipts,
        );
        Ok(if self.format == 1 {
            settings::digest(&fields)?
        } else {
            settings::digest(&(fields, self.format))?
        })
    }

    /// The format-4 upgrade rule. Call only on a record whose seal
    /// [`Record::check`] verified as stored; returns it at [`FORMAT`] and
    /// whether anything changed.
    ///
    /// An older record keeps its authored settings, pinned source and history,
    /// except that a legacy prose composer font resets to the mono default,
    /// and an unresolvable known embedded default is explicitly re-pinned.
    /// Authored sources are never rewritten or replaced by this migration.
    /// The effective interpretation is resolved again from the authored
    /// desktop and pinned source, and both digests are sealed anew.
    pub fn upgrade(mut self) -> anyhow::Result<(Self, bool)> {
        anyhow::ensure!(
            self.format <= FORMAT,
            "accepted format {} is newer than {FORMAT}",
            self.format
        );
        // This authored migration runs only after the stored seal verifies.
        // Keep receipt fences honest by advancing the profile revision.
        // A default re-pin below also advances the design revision.
        let retired_app = self.desktop.apps.remove("busviewer").is_some();
        if retired_app {
            eprintln!("settingsd: dropping retired apps.busviewer settings during profile upgrade");
        }
        let reset = self.desktop.email.reset_legacy_composer_family() || retired_app;
        let resolved = v1::resolve_with_embedded(&self.desktop, &self.embedded_source);
        let repin = resolved.is_err()
            && self.desktop.appearance.source.is_none()
            && RETIRED_EMBEDDED_DEFAULTS
                .contains(&settings::source_digest(&self.embedded_source).as_str());
        let effective = if repin {
            self.embedded_source = settings::EMBEDDED_DEFAULT_SOURCE.into();
            let effective = v1::resolve_with_embedded(&self.desktop, &self.embedded_source)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "accepted desktop cannot migrate to current embedded default: {e:?}"
                    )
                })?;
            eprintln!(
                "settingsd: re-pinning retired embedded default; dropping retired embedded tokens; preserving authored appearance overrides"
            );
            effective
        } else {
            resolved.map_err(|e| {
                anyhow::anyhow!("accepted desktop no longer resolves (authored sources require explicit repair): {e:?}")
            })?
        };
        if reset || repin {
            self.revision = Revision(
                self.revision
                    .0
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("accepted revision exhausted"))?,
            );
        }
        if repin {
            self.design_revision = self.revision;
        }
        if self.format == FORMAT && !reset && !repin {
            return Ok((self, false));
        }
        self.effective_digest = settings::digest(&effective)?;
        self.format = FORMAT;
        self.seal()?;
        Ok((self, true))
    }
    pub fn seal(&mut self) -> anyhow::Result<()> {
        self.content_digest = self.digest()?;
        Ok(())
    }
    /// The sealed format-4 effective values, re-derived by [`settings::v1`].
    pub fn effective(&self) -> anyhow::Result<std::collections::BTreeMap<String, Effective>> {
        let effective = v1::resolve_with_embedded(&self.desktop, &self.embedded_source)
            .map_err(|e| anyhow::anyhow!("accepted interpretation unsupported: {e:?}"))?;
        anyhow::ensure!(
            self.effective_digest == settings::digest(&effective)?,
            "accepted interpretation changed; explicit migration required"
        );
        Ok(effective)
    }
    pub fn check(&self, binding: &Binding) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema == SCHEMA, "unsupported accepted schema");
        anyhow::ensure!(&self.binding == binding, "wrong stored authority binding");
        anyhow::ensure!(
            self.embedded_source.len() <= MAX_SOURCE_BYTES,
            "pinned source limit exceeded"
        );
        anyhow::ensure!(
            self.content_digest == self.digest()?,
            "accepted content digest mismatch"
        );
        crate::store::check_lineage(
            binding,
            &self.incarnation,
            self.revision,
            self.design_revision,
            &self.receipts,
        )
    }
}
