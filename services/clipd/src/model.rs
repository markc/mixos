// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const TEXT_CAP: usize = 1024 * 1024;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const CHUNK: usize = 64 * 1024;
pub const HISTORY_LEN: usize = 50;
pub const HISTORY_PREVIEW_CHARS: usize = 512;
pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";
pub const MARKER: &str = "application/x-mixos-clipboard-";

pub fn image_mime(mime: &str) -> bool {
    mime.trim()
        .split_once('/')
        .is_some_and(|(kind, _)| kind.trim().eq_ignore_ascii_case("image"))
}

pub fn sensitive_hint(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).map_or(true, |value| {
        value
            .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
            .eq_ignore_ascii_case("secret")
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub origin_session: String,
    /// UUIDs survive daemon restarts without reusing an origin's generation.
    pub generation: String,
}

impl Stamp {
    pub fn new(session: &str) -> Self {
        Self {
            origin_session: session.into(),
            generation: uuid::Uuid::now_v7().to_string(),
        }
    }

    pub fn marker(&self) -> String {
        format!("{MARKER}{}-{}", self.origin_session, self.generation)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Get {
    pub mime: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub mime: String,
    pub data: String,
    #[serde(default)]
    pub primary: bool,
    /// `utf8` (default) or `base64` for arbitrary bytes.
    #[serde(default = "utf8")]
    pub encoding: String,
    #[serde(default)]
    pub sensitive: bool,
    pub stamp: Option<Stamp>,
}
fn utf8() -> String {
    "utf8".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clear {
    #[serde(default)]
    pub primary: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    pub n: usize,
    /// A picker may fence an index against intervening history changes.
    pub expected_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Push {
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub mime: String,
    pub data: String,
}

/// Bounded wire view; pick always uses the complete entry held by the engine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPreview {
    pub id: String,
    pub mime: String,
    pub data: String,
    pub size: usize,
    pub truncated: bool,
}

/// The only clipboard fields permitted in publications and status replies.
#[derive(Clone, Debug, Serialize)]
pub struct Metadata {
    pub session: String,
    pub generation: Option<String>,
    pub origin_session: Option<String>,
    pub mime_types: Vec<String>,
    pub size: Option<usize>,
    pub sensitive: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub session: String,
    pub stamp: Stamp,
}

#[derive(Clone)]
pub struct Selection {
    pub stamp: Stamp,
    pub mime_types: Vec<String>,
    pub sensitive: bool,
    pub size: Option<usize>,
    pub data: BTreeMap<String, Arc<Vec<u8>>>,
    pub offer: Option<crate::wayland::Offer>,
    pub remote: Option<Remote>,
}

impl Selection {
    pub fn owned(stamp: Stamp, mime: String, data: Vec<u8>, sensitive: bool) -> Self {
        let sensitive =
            sensitive || (mime.eq_ignore_ascii_case(SECRET_HINT) && sensitive_hint(&data));
        let mut items = BTreeMap::from([(mime.clone(), Arc::new(data))]);
        let mut types = vec![mime];
        if sensitive {
            let hint = types
                .iter()
                .find(|m| m.eq_ignore_ascii_case(SECRET_HINT))
                .cloned()
                .unwrap_or_else(|| {
                    types.push(SECRET_HINT.into());
                    SECRET_HINT.into()
                });
            items.insert(hint, Arc::new(b"secret".to_vec()));
        }
        Self {
            stamp,
            mime_types: types,
            sensitive,
            size: Some(items.values().map(|v| v.len()).sum()),
            data: items,
            offer: None,
            remote: None,
        }
    }

    pub fn text_mime(&self) -> Option<String> {
        [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "TEXT",
            "STRING",
        ]
        .into_iter()
        .find(|mime| self.mime_types.iter().any(|m| m == mime))
        .map(str::to_owned)
    }

    pub fn hint_mime(&self) -> Option<String> {
        self.mime_types
            .iter()
            .find(|mime| mime.eq_ignore_ascii_case(SECRET_HINT))
            .cloned()
    }

    pub fn can_export(&self, images: bool) -> Result<()> {
        ensure!(
            !self.sensitive,
            "SENSITIVE: selection cannot leave this session"
        );
        ensure!(
            images || !self.mime_types.iter().any(|m| image_mime(m)),
            "IMAGES_DISABLED: enable images for this pair first"
        );
        Ok(())
    }

    pub fn cache(&mut self, mime: String, bytes: Vec<u8>) -> Result<Arc<Vec<u8>>> {
        let used: usize = self
            .data
            .iter()
            .filter(|(m, _)| **m != mime)
            .map(|(_, b)| b.len())
            .sum();
        ensure!(
            used.saturating_add(bytes.len()) <= MAX_BYTES,
            "SIZE_CAP: selection cache exceeds 64 MiB"
        );
        let bytes = Arc::new(bytes);
        self.data.insert(mime, bytes.clone());
        Ok(bytes)
    }
}

pub struct Clipboard {
    pub session: String,
    pub selections: [Option<Selection>; 2],
    pub history: VecDeque<HistoryEntry>,
    seen: VecDeque<Stamp>,
    pub rotate: usize,
    /// Wayland events at or before an acknowledged ownership change are stale.
    pub observed: [u64; 2],
}

impl Clipboard {
    pub fn new(session: String) -> Self {
        Self {
            session,
            selections: [None, None],
            history: VecDeque::new(),
            seen: VecDeque::new(),
            rotate: 0,
            observed: [0, 0],
        }
    }

    pub fn is_echo(&self, stamp: &Stamp) -> bool {
        self.seen.contains(stamp) || self.selections.iter().flatten().any(|s| &s.stamp == stamp)
    }

    pub fn history_view(&self) -> Vec<HistoryPreview> {
        self.history
            .iter()
            .map(|entry| HistoryPreview {
                id: entry.id.clone(),
                mime: entry.mime.clone(),
                data: entry.data.chars().take(HISTORY_PREVIEW_CHARS).collect(),
                size: entry.data.len(),
                truncated: entry.data.chars().nth(HISTORY_PREVIEW_CHARS).is_some(),
            })
            .collect()
    }

    pub fn metadata(&self, primary: bool) -> Metadata {
        let selection = self.selections[usize::from(primary)].as_ref();
        Metadata {
            session: self.session.clone(),
            generation: selection.map(|s| s.stamp.generation.clone()),
            origin_session: selection.map(|s| s.stamp.origin_session.clone()),
            mime_types: selection.map(|s| s.mime_types.clone()).unwrap_or_default(),
            size: selection.and_then(|s| s.size),
            sensitive: selection.is_some_and(|s| s.sensitive),
            primary,
        }
    }

    pub fn adopt(&mut self, primary: bool, selection: Selection) -> bool {
        if self.is_echo(&selection.stamp) {
            return false;
        }
        self.seen.push_back(selection.stamp.clone());
        if self.seen.len() > 256 {
            self.seen.pop_front();
        }
        self.selections[usize::from(primary)] = Some(selection);
        self.rotate = 0;
        true
    }

    pub fn remember(&mut self, primary: bool) {
        let Some(s) = &self.selections[usize::from(primary)] else {
            return;
        };
        if s.sensitive {
            return;
        }
        let Some(mime) = s.text_mime() else {
            return;
        };
        let Some(bytes) = s.data.get(&mime) else {
            return;
        };
        if bytes.len() > TEXT_CAP || bytes.is_empty() {
            return;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        if self.history.front().is_some_and(|e| e.data == text) {
            return;
        }
        self.history.retain(|e| e.data != text);
        self.history.push_front(HistoryEntry {
            id: uuid::Uuid::now_v7().to_string(),
            mime,
            data: text.into(),
        });
        self.history.truncate(HISTORY_LEN);
    }
}

pub fn session(value: &str) -> Result<String> {
    let number = value
        .strip_prefix("vt")
        .or_else(|| value.strip_prefix("desk"))
        .unwrap_or("");
    if ["1", "3", "4"].contains(&number) {
        Ok(format!("vt{number}"))
    } else {
        bail!("SESSION: expected vt1, vt3, vt4 or the corresponding desk-prefixed alias")
    }
}

pub fn validate_mimes(types: &[String]) -> Result<()> {
    ensure!(
        !types.is_empty() && types.len() <= 128,
        "MIME: expected 1..=128 types"
    );
    for mime in types {
        ensure!(
            !mime.is_empty()
                && mime.len() <= 255
                && !mime.chars().any(char::is_control)
                && !mime.starts_with(MARKER),
            "MIME: invalid or reserved type"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(c: &Clipboard, data: String, sensitive: bool) -> Selection {
        Selection::owned(
            Stamp::new(&c.session),
            "text/plain".into(),
            data.into_bytes(),
            sensitive,
        )
    }
    #[test]
    fn loops_are_ignored_and_generations_survive_restarts() {
        let mut c = Clipboard::new("vt1".into());
        let s = text(&c, "one".into(), false);
        assert!(c.adopt(false, s.clone()));
        assert!(!c.adopt(false, s.clone()));
        assert!(c.adopt(false, text(&c, "two".into(), false)));
        assert!(!c.adopt(false, s));
        let current = c.selections[0].as_ref().unwrap().stamp.clone();
        for n in 0..260 {
            c.adopt(true, text(&c, n.to_string(), false));
        }
        assert!(
            c.is_echo(&current),
            "PRIMARY churn must not evict the live clipboard guard"
        );
        assert_ne!(Stamp::new("vt1"), Stamp::new("vt1"));
    }
    #[test]
    fn secrets_never_enter_history_or_exports() {
        let mut c = Clipboard::new("vt1".into());
        let s = text(&c, "password".into(), true);
        assert!(s.can_export(true).is_err());
        assert_eq!(s.data[SECRET_HINT].as_slice(), b"secret");
        c.adopt(false, s);
        c.remember(false);
        assert!(c.history.is_empty());
        for value in [b"secret".as_slice(), b"SECRET\n", b"secret\0", &[255]] {
            assert!(sensitive_hint(value));
        }
        assert!(!sensitive_hint(b"public"));
        let hint_only = Selection::owned(
            Stamp::new("vt1"),
            SECRET_HINT.into(),
            b"secret".to_vec(),
            true,
        );
        assert_eq!(hint_only.mime_types, vec![SECRET_HINT]);
        let implicit = Selection::owned(
            Stamp::new("vt1"),
            SECRET_HINT.into(),
            b"secret".to_vec(),
            false,
        );
        assert!(implicit.sensitive);
        assert!(implicit.can_export(true).is_err());
    }
    #[test]
    fn history_is_a_deduplicated_fifty_entry_text_ring() {
        let mut c = Clipboard::new("vt1".into());
        for n in 0..55 {
            c.adopt(false, text(&c, n.to_string(), false));
            c.remember(false);
        }
        assert_eq!(c.history.len(), HISTORY_LEN);
        assert_eq!(c.history.back().unwrap().data, "5");
        c.adopt(false, text(&c, "54".into(), false));
        c.remember(false);
        assert_eq!(c.history.len(), HISTORY_LEN);
        c.adopt(
            false,
            Selection::owned(Stamp::new("vt1"), "image/png".into(), vec![0], false),
        );
        c.remember(false);
        assert_eq!(c.history.front().unwrap().data, "54");
    }
    #[test]
    fn caps_and_image_pair_policy_are_enforced() {
        let mut c = Clipboard::new("vt1".into());
        c.adopt(false, text(&c, "x".repeat(TEXT_CAP + 1), false));
        c.remember(false);
        assert!(c.history.is_empty());
        let s = Selection::owned(Stamp::new("vt1"), "image/png".into(), vec![0], false);
        assert!(s.can_export(false).is_err());
        assert!(s.can_export(true).is_ok());
        let upper = Selection::owned(Stamp::new("vt1"), "IMAGE/PNG".into(), vec![0], false);
        assert!(upper.can_export(false).is_err());
        let mut cached = s;
        assert!(
            cached
                .cache("text/plain".into(), vec![0; MAX_BYTES + 1])
                .is_err()
        );
        assert!(!cached.data.contains_key("text/plain"));
        assert!(session("other-node").is_err());
    }

    #[test]
    fn history_wire_previews_are_bounded_without_truncating_stored_text() {
        let mut c = Clipboard::new("vt1".into());
        let original = "λ".repeat(600);
        c.adopt(false, text(&c, original.clone(), false));
        c.remember(false);
        let rows = c.history_view();
        assert_eq!(rows[0].data.chars().count(), HISTORY_PREVIEW_CHARS);
        assert_eq!(rows[0].size, original.len());
        assert!(rows[0].truncated);
        assert_eq!(c.history[0].data, original);
        assert!(serde_json::to_vec(&rows).unwrap().len() < 4096);
    }

    #[test]
    fn eager_text_enters_history_even_when_other_imported_mimes_are_lazy() {
        let mut c = Clipboard::new("vt4".into());
        let mut imported = Selection::owned(
            Stamp::new("vt1"),
            "text/plain".into(),
            b"imported text".to_vec(),
            false,
        );
        imported.mime_types.push("TEXT".into());
        imported.remote = Some(Remote {
            session: "vt1".into(),
            stamp: imported.stamp.clone(),
        });
        c.adopt(false, imported);
        c.remember(false);
        assert_eq!(c.history[0].data, "imported text");
    }

    #[test]
    fn metadata_never_serialises_clipboard_contents() {
        let mut c = Clipboard::new("vt1".into());
        c.adopt(false, text(&c, "must-not-escape".into(), true));
        let metadata = serde_json::to_value(c.metadata(false)).unwrap();
        assert_eq!(metadata["sensitive"], true);
        assert_eq!(metadata["primary"], false);
        assert!(metadata.get("data").is_none());
        assert_eq!(
            metadata
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "generation",
                "mime_types",
                "origin_session",
                "primary",
                "sensitive",
                "session",
                "size"
            ]
        );
        assert!(!metadata.to_string().contains("must-not-escape"));
    }
}
