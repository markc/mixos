// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
    /// `auto` (session detection), `csd` (the app) or `ssd` (the compositor).
    #[serde(skip_serializing_if = "is_default_decorations")]
    pub decorations: String,
    /// Window controls: auto follows the style; explicit positions survive upgrades.
    #[serde(skip_serializing_if = "is_default_caption_side")]
    pub caption_side: String,
    /// The server-side furniture preset; unknown names render as the default.
    #[serde(skip_serializing_if = "is_default_window_style")]
    pub window_style: String,
    #[serde(skip_serializing_if = "is_default_menu_style")]
    pub menu_style: String,
    #[serde(skip_serializing_if = "is_default_menu_side")]
    pub menu_side: String,
    #[serde(
        default = "default_actions_place",
        skip_serializing_if = "is_default_actions_place"
    )]
    pub actions_place: String,
    #[serde(
        default = "default_actions_align",
        skip_serializing_if = "is_default_actions_align"
    )]
    pub actions_align: String,
    #[serde(
        default = "default_title_align",
        skip_serializing_if = "is_default_title_align"
    )]
    pub title_align: String,
    /// The session icon stroke on Lucide's 24-unit grid, in quarter steps
    /// from [`ICON_STROKE_MIN`] to [`ICON_STROKE_MAX`]. None is the design's
    /// own `icon.stroke_width` token. Omitted at the default (see above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_stroke: Option<f64>,
    /// A UI family name, or None for the bundled Inter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// A CSS weight, 100 to 900 in steps of 100, or None for the scheme's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u16>,
    /// The interface text size in logical pixels, [`FONT_SIZE_MIN`] to
    /// [`FONT_SIZE_MAX`], or None for the design's own UI size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// A monospace family name, or None for the bundled JetBrains Mono.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_family: Option<String>,
    /// None selects the profile-pinned package source; Some is complete strict data.
    pub source: Option<String>,
}

/// The icon stroke range, in grid units, and its step.
pub const ICON_STROKE_MIN: f64 = 1.0;
pub const ICON_STROKE_MAX: f64 = 3.0;
pub const ICON_STROKE_STEP: f64 = 0.25;
impl Default for Appearance {
    fn default() -> Self {
        Self {
            scheme: "studio".into(),
            style: None,
            mode: "dark".into(),
            contrast: "normal".into(),
            decorations: "auto".into(),
            caption_side: "auto".into(),
            window_style: default_window_style(),
            menu_style: default_menu_style(),
            menu_side: default_menu_side(),
            actions_place: default_actions_place(),
            actions_align: default_actions_align(),
            title_align: default_title_align(),
            icon_stroke: None,
            font_family: None,
            font_weight: None,
            font_size: None,
            mono_family: None,
            source: None,
        }
    }
}

/// The interface text size range, in logical pixels.
pub const FONT_SIZE_MIN: f64 = 9.0;
pub const FONT_SIZE_MAX: f64 = 18.0;
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
pub(crate) fn deserialize_integer_u32<'de, D: serde::Deserializer<'de>>(
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
    /// The person's key bindings ([`crate::keymap`]). Omitted while empty,
    /// so a profile written before it re-serialises as it was written.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keymaps: crate::keymap::Keymaps,
    /// The terminal's preferences ([`crate::terminal`]). Omitted at its
    /// defaults, as `keymaps` is while empty.
    #[serde(skip_serializing_if = "crate::terminal::Terminal::is_default")]
    pub terminal: crate::terminal::Terminal,
    /// The email app's accounts and options ([`crate::email`]); nothing
    /// secret. Omitted at its defaults, as `terminal` is.
    #[serde(skip_serializing_if = "crate::email::Email::is_default")]
    pub email: crate::email::Email,
    /// The Ced app's editor style ([`crate::content`]). Omitted at its
    /// default, as `email` is.
    #[serde(skip_serializing_if = "crate::content::Ced::is_default")]
    pub ced: crate::content::Ced,
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
    /// Session scale, density and motion, from the same fenced read.
    #[serde(default)]
    pub ui: CommonUi,
    /// The person's key bindings, per application; omitted while empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keymaps: crate::keymap::Keymaps,
    /// The terminal's preferences; omitted at their defaults.
    #[serde(default, skip_serializing_if = "crate::terminal::Terminal::is_default")]
    pub terminal: crate::terminal::Terminal,
    /// The email app's accounts and options; omitted at their defaults.
    #[serde(default, skip_serializing_if = "crate::email::Email::is_default")]
    pub email: crate::email::Email,
    /// The Ced app's editor style; omitted at its default.
    #[serde(default, skip_serializing_if = "crate::content::Ced::is_default")]
    pub ced: crate::content::Ced,
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
            ui: snapshot.desktop.ui.clone(),
            keymaps: snapshot.desktop.keymaps.clone(),
            terminal: snapshot.desktop.terminal.clone(),
            email: snapshot.desktop.email.clone(),
            ced: snapshot.desktop.ced.clone(),
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
    // This projection carries the framing request. Each app resolves it
    // against its own window backend before reporting its effective look.
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
        default = "default_window_style",
        skip_serializing_if = "is_default_window_style"
    )]
    pub window_style: String,
    #[serde(
        default = "default_menu_style",
        skip_serializing_if = "is_default_menu_style"
    )]
    pub menu_style: String,
    #[serde(
        default = "default_menu_side",
        skip_serializing_if = "is_default_menu_side"
    )]
    pub menu_side: String,
    #[serde(
        default = "default_actions_place",
        skip_serializing_if = "is_default_actions_place"
    )]
    pub actions_place: String,
    #[serde(
        default = "default_actions_align",
        skip_serializing_if = "is_default_actions_align"
    )]
    pub actions_align: String,
    #[serde(
        default = "default_title_align",
        skip_serializing_if = "is_default_title_align"
    )]
    pub title_align: String,
    // The session type and stroke choices, omitted when unset (see above).
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
    // Key space v2 (§4.3), omitted where the format-4 path has nothing, so
    // its records and digests are unchanged: the accent the context draws
    // (sealed here, no longer a sidecar), the roundness preset (omitted at
    // `round`) and the authored `tune.*` values by tunable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<design::SrgbColour>,
    #[serde(
        default = "default_roundness",
        skip_serializing_if = "is_default_roundness"
    )]
    pub roundness: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tune: BTreeMap<String, serde_json::Value>,
    pub ui: CommonUi,
    pub design: design::DesignReadProjection,
    pub provenance: BTreeMap<String, String>,
}
pub(crate) fn default_roundness() -> String {
    design::geometry::Roundness::default().name().into()
}
pub(crate) fn is_default_roundness(value: &str) -> bool {
    value == design::geometry::Roundness::default().name()
}
pub(crate) fn default_decorations() -> String {
    Appearance::default().decorations
}
pub(crate) fn default_caption_side() -> String {
    Appearance::default().caption_side
}
pub(crate) fn is_default_decorations(value: &str) -> bool {
    value == "auto"
}
pub(crate) fn is_default_caption_side(value: &str) -> bool {
    value == "auto"
}
pub(crate) fn default_window_style() -> String {
    design::WindowStyle::default().name().into()
}
pub(crate) fn is_default_window_style(value: &str) -> bool {
    value == design::WindowStyle::default().name()
}
pub(crate) fn default_menu_style() -> String {
    design::MenuStyle::default().name().into()
}
pub(crate) fn is_default_menu_style(value: &str) -> bool {
    value == design::MenuStyle::default().name()
}
pub(crate) fn default_menu_side() -> String {
    design::MenuSide::default().name().into()
}
pub(crate) fn is_default_menu_side(value: &str) -> bool {
    value == design::MenuSide::default().name()
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

pub(crate) fn default_title_align() -> String {
    design::TitleAlign::default().name().into()
}
pub(crate) fn is_default_title_align(value: &str) -> bool {
    value == design::TitleAlign::default().name()
}

pub(crate) fn default_actions_align() -> String {
    design::ActionsAlign::default().name().into()
}
pub(crate) fn is_default_actions_align(value: &str) -> bool {
    value == design::ActionsAlign::default().name()
}

pub(crate) fn default_actions_place() -> String {
    design::ActionsPlace::default().name().into()
}
pub(crate) fn is_default_actions_place(value: &str) -> bool {
    value == design::ActionsPlace::default().name()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_decorations_default_to_auto_and_explicit_csd_survives_roundtrip() {
        let old: Desktop = serde_json::from_str(r#"{"appearance":{}}"#).unwrap();
        assert_eq!(old.appearance.decorations, "auto");
        let explicit: Desktop =
            serde_json::from_str(r#"{"appearance":{"decorations":"csd"}}"#).unwrap();
        assert_eq!(explicit.appearance.decorations, "csd");
        let saved = serde_json::to_value(&explicit).unwrap();
        assert_eq!(saved["appearance"]["decorations"], "csd");
        let loaded: Desktop = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.appearance.decorations, "csd");
        assert!(
            serde_json::to_value(&old).unwrap()["appearance"]
                .get("decorations")
                .is_none()
        );
    }
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
