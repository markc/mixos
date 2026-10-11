// SPDX-License-Identifier: MIT OR Apache-2.0
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// JSON/Mix integers must retain precision beyond 2^53. Numeric input refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(pub u64);
impl Serialize for Revision {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(serde::de::Error::custom(
                "revision must be canonical decimal u64 string",
            ));
        }
        text.parse::<u64>()
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub instance: String,
    pub profile: String,
}
impl Binding {
    pub fn validate(&self) -> Result<(), Diagnostic> {
        for (path, value) in [("instance", &self.instance), ("profile", &self.profile)] {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(Diagnostic::new(
                    "invalid_binding",
                    path,
                    "Use 1–64 ASCII letters, digits, dash or underscore",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub scheme: String,
    /// None takes the scheme's own style; Some names one of the four.
    ///
    /// These three fields are omitted from the serialised form at their
    /// defaults. A profile written before they existed therefore
    /// re-serialises byte for byte as it was written, and its stored
    /// content and effective digests still verify: no migration, no
    /// second representation. A non-default value appears only when
    /// chosen, which only this code can do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    pub mode: String,
    pub contrast: String,
    /// `csd` (the app's title bar) or `ssd` (the compositor's).
    #[serde(skip_serializing_if = "is_default_decorations")]
    pub decorations: String,
    /// Where a client-side title bar puts its captions: `right` or `left`.
    #[serde(skip_serializing_if = "is_default_caption_side")]
    pub caption_side: String,
    /// None selects the profile-pinned package source; Some is complete strict data.
    pub source: Option<String>,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            scheme: "studio".into(),
            style: None,
            mode: "dark".into(),
            contrast: "normal".into(),
            decorations: "csd".into(),
            caption_side: "right".into(),
            source: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommonUi {
    pub density: f64,
    pub text_scale: f64,
    pub reduced_motion: bool,
}
impl Default for CommonUi {
    fn default() -> Self {
        Self {
            density: 1.0,
            text_scale: 1.0,
            reduced_motion: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Panel {
    pub edge: String,
    pub mode: String,
    #[serde(deserialize_with = "deserialize_integer_u32")]
    pub thickness: u32,
}
pub(crate) fn integer_u32(value: f64) -> Option<u32> {
    (value.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&value) && value.fract() == 0.0)
        .then_some(value as u32)
}
fn deserialize_integer_u32<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u32, D::Error> {
    integer_u32(f64::deserialize(deserializer)?)
        .ok_or_else(|| serde::de::Error::custom("expected an integral u32 number"))
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            edge: "bottom".into(),
            mode: "dock".into(),
            thickness: 40,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    pub panels: BTreeMap<String, Panel>,
    pub page_order: Vec<String>,
}
impl Default for Shell {
    fn default() -> Self {
        Self {
            panels: BTreeMap::from([("bottom".into(), Panel::default())]),
            page_order: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppOverride {
    pub scheme: Option<String>,
    pub mode: Option<String>,
    pub contrast: Option<String>,
    pub text_scale: Option<f64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Desktop {
    pub appearance: Appearance,
    pub ui: CommonUi,
    pub shell: Shell,
    pub apps: BTreeMap<String, AppOverride>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
}
impl Diagnostic {
    pub fn new(code: &str, path: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub binding: Binding,
    /// What `settings.get` answers with. Omitted (and never sent) at its
    /// default, so a request reads as it did before the field existed.
    #[serde(default, skip_serializing_if = "View::is_full")]
    pub view: View,
    /// The snapshot schema of a `full` answer: 1 (the default, never sent)
    /// or 2 ([`crate::compact`]). Other schemas are refused.
    #[serde(default = "schema_one", skip_serializing_if = "is_schema_one")]
    pub schema: u32,
}

fn schema_one() -> u32 {
    crate::SCHEMA
}

fn is_schema_one(schema: &u32) -> bool {
    *schema == crate::SCHEMA
}

/// `settings.get`'s answer: the complete [`Snapshot`], or the [`Summary`]
/// a plain follower needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    #[default]
    Full,
    Summary,
}

impl View {
    pub fn is_full(&self) -> bool {
        *self == View::Full
    }
}

/// A profile's identity, revisions and appearance names: what a plain
/// follower (an app taking the session look) needs, without the effective
/// design projections or a custom design source. It is the payload of
/// [`crate::summary_topic`] and `settings.get`'s `summary` view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    pub source_digest: String,
    /// The profile's appearance, with `source` always `None`: whether the
    /// profile names a design source of its own is `custom_source`.
    pub appearance: Appearance,
    pub custom_source: bool,
}

/// The summary schema; independent of the snapshot's [`crate::SCHEMA`].
pub const SUMMARY_SCHEMA: u32 = 1;

impl Summary {
    pub fn of(snapshot: &Snapshot) -> Self {
        let mut appearance = snapshot.desktop.appearance.clone();
        let custom_source = appearance.source.take().is_some();
        Self {
            schema: SUMMARY_SCHEMA,
            binding: snapshot.binding.clone(),
            incarnation: snapshot.incarnation.clone(),
            revision: snapshot.revision,
            design_revision: snapshot.design_revision,
            source_digest: snapshot.source_digest.clone(),
            appearance,
            custom_source,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub binding: Binding,
    pub expected_incarnation: String,
    pub expected_revision: Revision,
    pub operation_id: String,
    pub changes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub reset: Vec<String>,
    /// Optional supplied digest is checked; every receipt stores the computed digest.
    #[serde(default)]
    pub request_digest: Option<String>,
}
impl ApplyRequest {
    pub fn digest(&self) -> Result<String, serde_json::Error> {
        let mut canonical = self.clone();
        canonical.request_digest = None;
        crate::digest(&canonical)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Changed,
    Unchanged,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: String,
    pub request_digest: String,
    pub incarnation: String,
    pub revision: Revision,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effective {
    pub scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    pub mode: String,
    pub contrast: String,
    // Omitted at their defaults, as in [`Appearance`], so effective digests
    // from before these fields still verify.
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
    pub ui: CommonUi,
    pub design: design::DesignReadProjection,
    pub provenance: BTreeMap<String, String>,
}
pub(crate) fn default_decorations() -> String {
    Appearance::default().decorations
}
pub(crate) fn default_caption_side() -> String {
    Appearance::default().caption_side
}
pub(crate) fn is_default_decorations(value: &str) -> bool {
    value == "csd"
}
pub(crate) fn is_default_caption_side(value: &str) -> bool {
    value == "right"
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    pub source_digest: String,
    pub desktop: Desktop,
    pub effective: BTreeMap<String, Effective>,
}
impl Snapshot {
    pub fn encoded_len(&self) -> Result<usize, serde_json::Error> {
        Ok(serde_json::to_vec(self)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_keep_full_precision_and_refuse_noncanonical_input() {
        let value = Revision(u64::MAX);
        assert_eq!(
            serde_json::from_str::<Revision>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
        for input in [
            "9007199254740993",
            "\"01\"",
            "\"+1\"",
            "\"18446744073709551616\"",
        ] {
            assert!(serde_json::from_str::<Revision>(input).is_err());
        }
        assert_eq!(
            strict::from_str::<BTreeMap<String, Revision>>("{revision: \"9007199254740993\"}")
                .unwrap()["revision"]
                .0,
            9007199254740993
        );
    }
}
