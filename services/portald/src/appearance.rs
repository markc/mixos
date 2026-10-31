// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Portal values for the `org.freedesktop.appearance` namespace, from one
//! accepted [`Projection`]. settingsd computes everything (ruling 1): the
//! resolved mode and the `appearance.export_toolkits` gate come from its
//! `values` view, the contrast and the accent from its `settings.palette`
//! for that mode, both at one record identity. With the gate off the portal
//! states no preference (key space v2 §5.3: the gate covers GTK, Qt and the
//! portal). Pure data; the D-Bus surface lives in `portal`.
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use settings::{Binding, Diagnostic, Revision};
use std::collections::BTreeMap;
use zbus::zvariant::Value;

pub const NAMESPACE: &str = "org.freedesktop.appearance";
pub const COLOR_SCHEME: &str = "color-scheme";
pub const ACCENT_COLOR: &str = "accent-color";
pub const CONTRAST: &str = "contrast";
/// Every served key, in the order `ReadAll` lists them.
pub const KEYS: [&str; 3] = [COLOR_SCHEME, ACCENT_COLOR, CONTRAST];
pub const MAX_FILTERS: usize = 64;
pub const MAX_FILTER_BYTES: usize = 256;

/// The schema of [`Projection`], as the cache stores it.
pub const PROJECTION_SCHEMA: u32 = 4;
/// The `values` keys a projection reads beside the resolved mode.
pub const VALUE_KEYS: [&str; 1] = [EXPORT_KEY];
/// The gate: GTK, Qt and the portal follow settingsd only while it is true.
pub const EXPORT_KEY: &str = "appearance.export_toolkits";

pub fn valid_filters(filters: &[String]) -> bool {
    filters.len() <= MAX_FILTERS && filters.iter().all(|s| s.len() <= MAX_FILTER_BYTES)
}

/// What the portal publishes of one settingsd record: its identity, the
/// resolved mode, the contrast, the export gate and the palette's accent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    /// `dark` or `light`: settingsd's resolved mode, never `auto`.
    pub mode: String,
    /// `normal` or `high`.
    pub contrast: String,
    /// `appearance.export_toolkits`: false serves no preference.
    pub export: bool,
    /// sRGB, 0.0..=1.0 per channel, alpha excluded.
    pub accent: [f64; 3],
}

fn invalid(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("invalid_appearance", path, message)
}

impl Projection {
    /// The projection of one `settings.get {view: values}` answer and one
    /// `settings.palette {mode}` answer for the mode the first resolved.
    /// Both must be current, for `binding`, at the same incarnation and
    /// revision: anything else is refused, never mixed.
    pub fn from_answers(
        binding: &Binding,
        values: &Json,
        palette: &Json,
    ) -> Result<Self, Diagnostic> {
        for (name, answer) in [("values", values), ("palette", palette)] {
            if answer.get("status").and_then(Json::as_str) != Some("current") {
                return Err(invalid(name, "Authority did not return a current answer"));
            }
            let theirs: Binding = serde_json::from_value(answer["binding"].clone())
                .map_err(|e| invalid(name, e.to_string()))?;
            if &theirs != binding {
                return Err(Diagnostic::new(
                    "wrong_target",
                    "binding",
                    "Appearance binding differs",
                ));
            }
        }
        let incarnation = values["incarnation"]
            .as_str()
            .ok_or_else(|| invalid("incarnation", "Missing incarnation"))?;
        let revision: Revision = serde_json::from_value(values["revision"].clone())
            .map_err(|e| invalid("revision", e.to_string()))?;
        if palette["incarnation"].as_str() != Some(incarnation)
            || serde_json::from_value::<Revision>(palette["revision"].clone()).ok()
                != Some(revision)
        {
            return Err(Diagnostic::new(
                "moved",
                "revision",
                "The record moved between the values and palette reads",
            ));
        }
        let mode = values["resolved"]["mode"]
            .as_str()
            .ok_or_else(|| invalid("resolved.mode", "Missing resolved mode"))?;
        let ours = &palette["palette"][mode];
        let contrast = ours["contrast"]
            .as_str()
            .ok_or_else(|| invalid("palette.contrast", "Missing palette for the mode"))?;
        let accent: design::SrgbColour = serde_json::from_value(ours["accent"].clone())
            .map_err(|e| invalid("palette.accent", e.to_string()))?;
        // The key is always in the answer with its default filled; a value
        // that is not a boolean is refused rather than read as either.
        let export = match &values["values"][EXPORT_KEY] {
            Json::Bool(export) => *export,
            Json::Null => true,
            _ => return Err(invalid(EXPORT_KEY, "Expected a boolean")),
        };
        let projection = Self {
            schema: PROJECTION_SCHEMA,
            binding: binding.clone(),
            incarnation: incarnation.to_owned(),
            revision,
            mode: mode.to_owned(),
            contrast: contrast.to_owned(),
            export,
            accent: [accent.red, accent.green, accent.blue],
        };
        projection.validate()?;
        Ok(projection)
    }

    /// Refuse anything the portal must not publish. Every projection is
    /// validated where it is received, cached ones included.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.schema != PROJECTION_SCHEMA {
            return Err(Diagnostic::new(
                "unsupported_schema",
                "schema",
                format!("Expected appearance schema {PROJECTION_SCHEMA}"),
            ));
        }
        self.binding.validate()?;
        if !matches!(self.mode.as_str(), "dark" | "light") {
            return Err(invalid("mode", "Use dark or light"));
        }
        if !matches!(self.contrast.as_str(), "normal" | "high") {
            return Err(invalid("contrast", "Use normal or high"));
        }
        for (index, channel) in self.accent.iter().enumerate() {
            if !channel.is_finite() || !(0.0..=1.0).contains(channel) {
                return Err(invalid(
                    &format!("accent[{index}]"),
                    "Channel must be finite and within 0.0..=1.0",
                ));
            }
        }
        Ok(())
    }

    /// The record this projection was read from. Two projections with
    /// equal identity describe the same accepted state.
    pub fn identity(&self) -> (&str, Revision) {
        (&self.incarnation, self.revision)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    /// 1 dark, 2 light, 0 no preference (the export gate is off).
    pub color_scheme: u32,
    /// 0 normal, 1 high.
    pub contrast: u32,
    /// sRGB 0.0..=1.0 per channel. `None` means the key is not served.
    pub accent: Option<[f64; 3]>,
}

impl Values {
    /// Only used when neither settingsd nor a validated cache is available:
    /// dark and normal contrast, with no accent published.
    pub fn defaults() -> Self {
        Self {
            color_scheme: 1,
            contrast: 0,
            accent: None,
        }
    }

    /// The projection's mode, contrast and accent, as settingsd gave them;
    /// no preference, normal contrast and no accent with the gate off.
    pub fn from_projection(projection: &Projection) -> Self {
        if !projection.export {
            return Self {
                color_scheme: 0,
                contrast: 0,
                accent: None,
            };
        }
        Self {
            color_scheme: match projection.mode.as_str() {
                "light" => 2,
                _ => 1,
            },
            contrast: match projection.contrast.as_str() {
                "high" => 1,
                _ => 0,
            },
            accent: Some(projection.accent),
        }
    }

    /// The single-layer value of one key, or `None` when it is not served.
    pub fn get(&self, namespace: &str, key: &str) -> Option<Value<'static>> {
        if namespace != NAMESPACE {
            return None;
        }
        match key {
            COLOR_SCHEME => Some(Value::U32(self.color_scheme)),
            CONTRAST => Some(Value::U32(self.contrast)),
            ACCENT_COLOR => self.accent.map(|[r, g, b]| Value::from((r, g, b))),
            _ => None,
        }
    }

    /// Every served key whose namespace matches one of the globs.
    pub fn matching(&self, globs: &[String]) -> BTreeMap<String, BTreeMap<String, Value<'static>>> {
        let mut out = BTreeMap::new();
        if !valid_filters(globs)
            || !(globs.is_empty()
                || globs
                    .iter()
                    .any(|glob| glob.is_empty() || glob_match(glob, NAMESPACE)))
        {
            return out;
        }
        let mut keys = BTreeMap::new();
        for key in KEYS {
            if let Some(value) = self.get(NAMESPACE, key) {
                keys.insert(key.to_owned(), value);
            }
        }
        if !keys.is_empty() {
            out.insert(NAMESPACE.to_owned(), keys);
        }
        out
    }

    /// Keys whose value differs from `self`, with their new values. A key that
    /// stops being served has no value to announce, so it is not listed.
    pub fn changed(&self, next: &Self) -> Vec<(&'static str, Value<'static>)> {
        let mut out = Vec::new();
        if self.color_scheme != next.color_scheme {
            out.push((COLOR_SCHEME, Value::U32(next.color_scheme)));
        }
        if self.contrast != next.contrast {
            out.push((CONTRAST, Value::U32(next.contrast)));
        }
        if self.accent != next.accent
            && let Some(value) = next.get(NAMESPACE, ACCENT_COLOR)
        {
            out.push((ACCENT_COLOR, value));
        }
        out
    }
}

/// Iterative dynamic programming: O(pattern bytes * text bytes) time and
/// O(text bytes) space, with no recursive or combinatorial backtracking.
/// `*` matches any run, `?` one byte. Portal callers bound filter size/count.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > MAX_FILTER_BYTES {
        return false;
    }
    let mut matches = vec![false; text.len() + 1];
    matches[0] = true;
    for byte in pattern.bytes() {
        if byte == b'*' {
            for index in 1..=text.len() {
                matches[index] |= matches[index - 1];
            }
        } else {
            for index in (1..=text.len()).rev() {
                matches[index] =
                    matches[index - 1] && (byte == b'?' || byte == text.as_bytes()[index - 1]);
            }
            matches[0] = false;
        }
    }
    matches[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> Binding {
        Binding {
            instance: "host".into(),
            profile: "default".into(),
        }
    }

    fn projection(mode: &str, contrast: &str, accent: [f64; 3]) -> Projection {
        Projection {
            schema: PROJECTION_SCHEMA,
            binding: binding(),
            incarnation: "inc".into(),
            revision: Revision(1),
            mode: mode.into(),
            contrast: contrast.into(),
            export: true,
            accent,
        }
    }

    fn values(revision: &str, mode: &str, export: Json) -> Json {
        json!({"status":"current","binding":binding(),"incarnation":"inc","revision":revision,
            "values":{EXPORT_KEY:export},"resolved":{"mode":mode}})
    }

    fn palette(revision: &str, mode: &str, contrast: &str) -> Json {
        json!({"status":"current","binding":binding(),"incarnation":"inc","revision":revision,
            "palette":{mode:{"mode":mode,"contrast":contrast,"accent":"#336699FF"}}})
    }

    #[test]
    fn a_projection_takes_the_resolved_mode_and_the_palettes_contrast_and_accent() {
        let p = Projection::from_answers(
            &binding(),
            &values("4", "light", json!(true)),
            &palette("4", "light", "high"),
        )
        .unwrap();
        assert_eq!((p.mode.as_str(), p.contrast.as_str()), ("light", "high"));
        assert!(p.export);
        assert_eq!(p.revision, Revision(4));
        assert_eq!(p.accent, [0.2, 0.4, 0.6]);
        let off = Projection::from_answers(
            &binding(),
            &values("4", "dark", json!(false)),
            &palette("4", "dark", "normal"),
        )
        .unwrap();
        assert!(!off.export);
        assert!(
            Projection::from_answers(
                &binding(),
                &values("4", "dark", json!("yes")),
                &palette("4", "dark", "normal"),
            )
            .is_err()
        );
    }

    #[test]
    fn answers_at_two_revisions_or_for_another_binding_are_refused() {
        let moved = Projection::from_answers(
            &binding(),
            &values("4", "dark", json!(true)),
            &palette("5", "dark", "normal"),
        )
        .unwrap_err();
        assert_eq!(moved.code, "moved");
        let mut other = palette("4", "dark", "normal");
        other["binding"]["profile"] = json!("other");
        let wrong = Projection::from_answers(&binding(), &values("4", "dark", json!(true)), &other)
            .unwrap_err();
        assert_eq!(wrong.code, "wrong_target");
        // The palette must carry the resolved mode.
        assert!(
            Projection::from_answers(
                &binding(),
                &values("4", "light", json!(true)),
                &palette("4", "dark", "normal"),
            )
            .is_err()
        );
    }

    #[test]
    fn maps_mode_contrast_and_accent_to_portal_values() {
        let dark = Values::from_projection(&projection("dark", "normal", [0.1, 0.2, 0.3]));
        assert_eq!(dark.color_scheme, 1);
        assert_eq!(dark.contrast, 0);
        assert_eq!(dark.accent, Some([0.1, 0.2, 0.3]));
        let light = Values::from_projection(&projection("light", "high", [0.1, 0.2, 0.3]));
        assert_eq!(light.color_scheme, 2);
        assert_eq!(light.contrast, 1);
        assert!(matches!(
            light.get(NAMESPACE, ACCENT_COLOR),
            Some(Value::Structure(_))
        ));
    }

    #[test]
    fn with_the_export_gate_off_the_portal_states_no_preference() {
        let mut off = projection("light", "high", [0.1, 0.2, 0.3]);
        off.export = false;
        let values = Values::from_projection(&off);
        assert_eq!(values.get(NAMESPACE, COLOR_SCHEME), Some(Value::U32(0)));
        assert_eq!(values.get(NAMESPACE, CONTRAST), Some(Value::U32(0)));
        assert!(values.get(NAMESPACE, ACCENT_COLOR).is_none());
        let on = Values::from_projection(&projection("light", "high", [0.1, 0.2, 0.3]));
        let changes = on.changed(&values);
        assert_eq!(
            changes.len(),
            2,
            "color-scheme and contrast; the accent stops"
        );
    }

    #[test]
    fn defaults_serve_dark_normal_and_no_accent() {
        let defaults = Values::defaults();
        assert_eq!(defaults.get(NAMESPACE, COLOR_SCHEME), Some(Value::U32(1)));
        assert_eq!(defaults.get(NAMESPACE, CONTRAST), Some(Value::U32(0)));
        assert!(defaults.get(NAMESPACE, ACCENT_COLOR).is_none());
    }

    #[test]
    fn unknown_namespaces_and_keys_are_not_served() {
        let values = Values::defaults();
        assert!(values.get("org.gnome.desktop", COLOR_SCHEME).is_none());
        assert!(values.get(NAMESPACE, "font-name").is_none());
    }

    #[test]
    fn globs_select_namespaces() {
        assert!(glob_match("org.freedesktop.*", NAMESPACE));
        assert!(glob_match("*", NAMESPACE));
        assert!(glob_match("org.freedesktop.appearanc?", NAMESPACE));
        assert!(!glob_match("org.gnome.*", NAMESPACE));
        assert!(!glob_match("org.freedesktop", NAMESPACE));
        let values = Values::defaults();
        assert_eq!(
            values.matching(&["org.freedesktop.*".into()])[NAMESPACE].len(),
            2
        );
        assert!(values.matching(&["org.gnome.*".into()]).is_empty());
    }

    #[test]
    fn changed_lists_only_keys_whose_values_moved() {
        let base = Values::from_projection(&projection("dark", "normal", [0.1, 0.2, 0.3]));
        assert!(base.changed(&base.clone()).is_empty());
        let light = Values::from_projection(&projection("light", "normal", [0.1, 0.2, 0.3]));
        let changes = base.changed(&light);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, COLOR_SCHEME);
        assert_eq!(changes[0].1, Value::U32(2));
    }
}
