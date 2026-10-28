// SPDX-License-Identifier: MIT OR Apache-2.0
//! The lenient portal edge; the native files contract remains strict.
use crate::portal::PortalError;
use serde_json::{Value as Json, json};
use std::collections::{HashMap, HashSet};
use zbus::zvariant::{OwnedValue, Value};

pub type Options = HashMap<String, OwnedValue>;
pub type Filter = (String, Vec<(u32, String)>);
pub type Choice = (String, String, Vec<(String, String)>, String);
pub type Results = HashMap<&'static str, Value<'static>>;

fn adjusted(key: &str) {
    tracing::warn!(
        option = key,
        "FileChooser option adjusted to the files contract"
    );
}

pub fn cut(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn label(value: &str, limit: usize, key: &str) -> Option<String> {
    if value.is_empty() {
        adjusted(key);
        return None;
    }
    if value.len() > limit {
        adjusted(key);
    }
    Some(cut(value, limit))
}

// Conversion through serde retains the exact D-Bus signature check: e.g. a
// signed integer, or a string in place of ay, is never silently coerced.
fn option<T>(options: &Options, key: &str) -> Result<Option<T>, PortalError>
where
    T: TryFrom<OwnedValue>,
{
    options
        .get(key)
        .map(|v| {
            v.try_clone()
                .ok()
                .and_then(|v| T::try_from(v).ok())
                .ok_or_else(|| PortalError::InvalidArgument(format!("wrong D-Bus type for {key}")))
        })
        .transpose()
}

fn bytes(value: Vec<u8>) -> Result<Vec<u8>, PortalError> {
    if value.last() != Some(&0) || value[..value.len() - 1].contains(&0) {
        return Err(PortalError::InvalidArgument(
            "byte string requires exactly one trailing NUL".into(),
        ));
    }
    Ok(value[..value.len() - 1].to_vec())
}

pub fn percent(value: &[u8]) -> String {
    let mut out = String::new();
    for &b in value {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn location(value: Vec<u8>, key: &str) -> Result<Option<String>, PortalError> {
    let value = bytes(value)?;
    if value.first() != Some(&b'/') || value.len() > 4096 {
        adjusted(key);
        return Ok(None);
    }
    Ok(Some(format!("file://{}", percent(&value))))
}

fn name(value: &[u8]) -> Option<Json> {
    if value.is_empty()
        || value.len() > 255
        || value.contains(&b'/')
        || value.contains(&0)
        || value == b"."
        || value == b".."
    {
        return None;
    }
    Some(match std::str::from_utf8(value) {
        Ok(s) => json!(s),
        Err(_) => json!({"bytes": percent(value)}),
    })
}

fn filter((title, entries): Filter, key: &str) -> Option<Json> {
    let title = label(&title, 128, key)?;
    let mut patterns = Vec::new();
    let mut mime_types = Vec::new();
    if entries.len() > 512 {
        adjusted(key);
    }
    for (kind, text) in entries.into_iter().take(512) {
        if text.is_empty() || text.len() > 256 {
            adjusted(key);
            continue;
        }
        match kind {
            0 if glob::Pattern::new(&text).is_ok() => patterns.push(text),
            1 => mime_types.push(text),
            _ => adjusted(key),
        }
    }
    if patterns.is_empty() && mime_types.is_empty() {
        adjusted(key);
        return None;
    }
    Some(json!({"name": title, "patterns": patterns, "mime_types": mime_types}))
}

fn choices(values: Vec<Choice>) -> Vec<Json> {
    let mut ids = HashSet::new();
    if values.len() > 16 {
        adjusted("choices");
    }
    values
        .into_iter()
        .take(16)
        .filter_map(|(id, text, options, default)| {
            if id.is_empty() || id.len() > 64 || !ids.insert(id.clone()) {
                adjusted("choices");
                return None;
            }
            let text = label(&text, 128, "choices")?;
            let mut ids = HashSet::new();
            if options.len() > 32 {
                adjusted("choices.options");
            }
            let options: Vec<_> = options
                .into_iter()
                .take(32)
                .filter_map(|(id, text)| {
                    if id.is_empty() || id.len() > 64 || !ids.insert(id.clone()) {
                        adjusted("choices.options");
                        return None;
                    }
                    Some(json!({"id": id, "label": label(&text, 128, "choices.options")?}))
                })
                .collect();
            let valid = if options.is_empty() {
                default == "true" || default == "false"
            } else {
                options.iter().any(|o| o["id"] == default)
            };
            let mut choice = json!({"id": id, "label": text, "options": options});
            if valid {
                choice["default"] = json!(default);
            } else if !default.is_empty() {
                adjusted("choices.default");
            }
            Some(choice)
        })
        .collect()
}

pub fn map(mode: &str, parent: &str, title: &str, options: &Options) -> Result<Json, PortalError> {
    // Check every known option even when this method ignores its value.
    let accept = option::<String>(options, "accept_label")?;
    let _modal = option::<bool>(options, "modal")?;
    let multiple = option::<bool>(options, "multiple")?;
    let directory = option::<bool>(options, "directory")?;
    let filters = option::<Vec<Filter>>(options, "filters")?;
    let current = option::<Filter>(options, "current_filter")?;
    let choice = option::<Vec<Choice>>(options, "choices")?;
    let current_name = option::<String>(options, "current_name")?;
    let folder = option::<Vec<u8>>(options, "current_folder")?
        .map(|v| location(v, "current_folder"))
        .transpose()?
        .flatten();
    let file = option::<Vec<u8>>(options, "current_file")?
        .map(|v| location(v, "current_file"))
        .transpose()?
        .flatten();
    let files = option::<Vec<Vec<u8>>>(options, "files")?
        .map(|values| values.into_iter().map(bytes).collect::<Result<Vec<_>, _>>())
        .transpose()?;
    let mut body = json!({"mode": mode, "parent_window": cut(parent, 256)});
    if parent.len() > 256 {
        adjusted("parent_window");
    }
    if let Some(title) = label(title, 256, "title") {
        body["title"] = json!(title);
    }
    if let Some(accept) = accept {
        let plain = accept.replace('_', "");
        if plain != accept {
            adjusted("accept_label");
        }
        if let Some(accept) = label(&plain, 64, "accept_label") {
            body["accept_label"] = json!(accept);
        }
    }
    if mode == "open" {
        if let Some(value) = multiple {
            body["multiple"] = json!(value);
        }
        if let Some(value) = directory {
            body["directory"] = json!(value);
        }
    } else {
        if multiple.is_some() {
            adjusted("multiple");
        }
        if directory.is_some() {
            adjusted("directory");
        }
    }
    if mode != "save_files" {
        if let Some(filters) = filters {
            if filters.len() > 64 {
                adjusted("filters");
            }
            body["filters"] = json!(
                filters
                    .into_iter()
                    .take(64)
                    .filter_map(|f| filter(f, "filters"))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(current) = current.and_then(|f| filter(f, "current_filter")) {
            body["current_filter"] = current;
        }
    } else {
        if filters.is_some() {
            adjusted("filters");
        }
        if current.is_some() {
            adjusted("current_filter");
        }
    }
    if let Some(values) = choice {
        body["choices"] = json!(choices(values));
    }
    if let Some(value) = folder {
        body["current_folder"] = json!(value);
    }
    if mode == "save" {
        if let Some(value) = file {
            body["current_file"] = json!(value);
        }
        if let Some(value) = current_name {
            if let Some(value) = name(value.as_bytes()) {
                body["current_name"] = value;
            } else {
                adjusted("current_name");
            }
        }
    } else {
        if file.is_some() {
            adjusted("current_file");
        }
        if current_name.is_some() {
            adjusted("current_name");
        }
    }
    if mode == "save_files" {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for value in files.unwrap_or_default() {
            let tail = value.rsplit(|&b| b == b'/').next().unwrap_or_default();
            if tail != value {
                adjusted("files");
            }
            if let Some(value) = name(tail) {
                if names.len() < 256 && seen.insert(tail.to_vec()) {
                    names.push(value);
                } else {
                    adjusted("files");
                }
            } else {
                adjusted("files");
            }
        }
        if names.is_empty() {
            return Err(PortalError::InvalidArgument(
                "SaveFiles requires usable files".into(),
            ));
        }
        body["files"] = json!(names);
    } else if files.is_some() {
        adjusted("files");
    }
    // Reserve space for correlation and on_behalf_of, added by the caller.
    // Large legal filter lists can otherwise exceed the native 256 KiB body.
    while serde_json::to_vec(&body).expect("JSON").len() > 250 * 1024 {
        if let Some(filters) = body.get_mut("filters").and_then(Json::as_array_mut).filter(|v| !v.is_empty()) {
            filters.pop();
            adjusted("filters");
        } else if let Some(values) = body.get_mut("choices").and_then(Json::as_array_mut).filter(|v| !v.is_empty()) {
            values.pop(); adjusted("choices");
        } else if let Some(values) = body.get_mut("current_filter").and_then(|v| v.get_mut("patterns")).and_then(Json::as_array_mut).filter(|v| v.len() > 1) {
            values.pop(); adjusted("current_filter");
        } else if let Some(values) = body.get_mut("current_filter").and_then(|v| v.get_mut("mime_types")).and_then(Json::as_array_mut).filter(|v| v.len() > 1) {
            values.pop(); adjusted("current_filter");
        } else if let Some(values) = body.get_mut("files").and_then(Json::as_array_mut).filter(|v| v.len() > 1) {
            values.pop(); adjusted("files");
        } else {
            // One bounded name/filter entry and the fixed fields always fit.
            unreachable!("normalised FileChooser body exceeds budget");
        }
    }
    Ok(body)
}

fn wire_filter(value: &Json) -> Option<Filter> {
    let mut entries = Vec::new();
    for (key, kind) in [("patterns", 0), ("mime_types", 1)] {
        for text in value[key].as_array()? {
            entries.push((kind, text.as_str()?.to_owned()));
        }
    }
    Some((value["name"].as_str()?.to_owned(), entries))
}

pub fn outcome(value: &Json, had_choices: bool) -> (u32, Results) {
    let code = value["response"].as_u64().filter(|v| *v <= 2).unwrap_or(2) as u32;
    let mut results = HashMap::new();
    if code == 0 {
        let Some(uris) = value["uris"].as_array().and_then(|v| {
            v.iter()
                .map(|s| s.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        }) else {
            return (2, results);
        };
        results.insert("uris", Value::from(uris));
        if had_choices {
            let choices: Vec<(String, String)> = value["choices"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(id, v)| Some((id.clone(), v.as_str()?.to_owned())))
                .collect();
            results.insert("choices", Value::from(choices));
        }
        if let Some(filter) = wire_filter(&value["current_filter"]) {
            results.insert("current_filter", Value::from(filter));
        }
    }
    (code, results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(options: &mut Options, key: &str, value: Value<'_>) {
        options.insert(key.into(), OwnedValue::try_from(value).unwrap());
    }

    #[test]
    fn maps_every_option_and_round_trips_filters_and_choices() {
        let mut options = Options::new();
        put(&mut options, "handle_token", Value::from("token"));
        put(&mut options, "accept_label", Value::from("_Choose"));
        put(&mut options, "modal", Value::from(true));
        put(&mut options, "multiple", Value::from(true));
        put(&mut options, "directory", Value::from(true));
        let filter: Filter = (
            "Images".into(),
            vec![(0, "*.png".into()), (1, "image/jpeg".into())],
        );
        put(&mut options, "filters", Value::from(vec![filter.clone()]));
        put(&mut options, "current_filter", Value::from(filter.clone()));
        let choices: Vec<Choice> = vec![
            (
                "encoding".into(),
                "Encoding".into(),
                vec![("utf8".into(), "UTF-8".into())],
                "".into(),
            ),
            ("check".into(), "Check".into(), vec![], "true".into()),
        ];
        put(&mut options, "choices", Value::from(choices));
        put(
            &mut options,
            "current_folder",
            Value::from(b"/tmp/\xff\0".to_vec()),
        );
        put(&mut options, "ignored", Value::from(42u32));
        let body = map("open", "wayland:parent", "Title", &options).unwrap();
        assert_eq!(body["mode"], "open");
        assert_eq!(body["parent_window"], "wayland:parent");
        assert_eq!(body["title"], "Title");
        assert_eq!(body["accept_label"], "Choose");
        assert_eq!(body["multiple"], true);
        assert_eq!(body["directory"], true);
        assert_eq!(body["current_folder"], "file:///tmp/%FF");
        assert!(body.get("modal").is_none());
        assert!(body.get("handle_token").is_none());
        assert!(body.get("ignored").is_none());
        assert!(body["choices"][0].get("default").is_none());
        assert_eq!(body["choices"][1]["default"], "true");
        let event = json!({"response":0, "uris":["file:///tmp/%FF"], "choices":{"encoding":"utf8","check":"true"}, "current_filter":body["current_filter"]});
        let (code, mut results) = outcome(&event, true);
        assert_eq!(code, 0);
        assert_eq!(
            Filter::try_from(
                OwnedValue::try_from(results.remove("current_filter").unwrap()).unwrap()
            )
            .unwrap(),
            filter
        );
        let returned = Vec::<(String, String)>::try_from(
            OwnedValue::try_from(results.remove("choices").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            returned,
            vec![
                ("check".into(), "true".into()),
                ("encoding".into(), "utf8".into())
            ]
        );
        assert_eq!(
            Vec::<String>::try_from(OwnedValue::try_from(results.remove("uris").unwrap()).unwrap())
                .unwrap(),
            vec!["file:///tmp/%FF"]
        );
        for response in [1, 2] {
            let (code, results) = outcome(&json!({"response":response,"uris":["secret"]}), true);
            assert_eq!(code, response);
            assert!(results.is_empty());
        }
    }

    #[test]
    fn save_paths_and_save_files_keep_exact_bytes_and_mode_rules() {
        let mut options = Options::new();
        put(
            &mut options,
            "current_file",
            Value::from(b"/tmp/\xfe\0".to_vec()),
        );
        put(&mut options, "current_name", Value::from("draft.txt"));
        put(&mut options, "multiple", Value::from(true));
        put(&mut options, "directory", Value::from(false));
        let save = map("save", "", "", &options).unwrap();
        assert_eq!(save["current_file"], "file:///tmp/%FE");
        assert_eq!(save["current_name"], "draft.txt");
        for key in ["title", "multiple", "directory"] {
            assert!(save.get(key).is_none());
        }
        put(
            &mut options,
            "filters",
            Value::from(vec![("Text".to_owned(), vec![(0u32, "*.txt".to_owned())])]),
        );
        put(
            &mut options,
            "files",
            Value::from(vec![
                b"/folder/\xff\0".to_vec(),
                b"\xff\0".to_vec(),
                b"..\0".to_vec(),
                b"ok\0".to_vec(),
            ]),
        );
        let save = map("save_files", "x11:aa", "", &options).unwrap();
        assert_eq!(save["files"], json!([{"bytes":"%FF"}, "ok"]));
        for key in [
            "filters",
            "current_file",
            "current_name",
            "multiple",
            "directory",
        ] {
            assert!(save.get(key).is_none());
        }
    }

    #[test]
    fn refuses_only_wrong_types_nul_and_no_usable_files() {
        for key in [
            "accept_label",
            "modal",
            "multiple",
            "directory",
            "filters",
            "current_filter",
            "choices",
            "current_folder",
            "current_file",
            "current_name",
            "files",
        ] {
            let mut options = Options::new();
            put(&mut options, key, Value::from(99u32));
            assert!(map("open", "", "", &options).is_err(), "{key}");
        }
        for value in [b"/tmp".to_vec(), b"/tmp\0x\0".to_vec(), vec![]] {
            let mut options = Options::new();
            put(&mut options, "current_file", Value::from(value));
            assert!(map("open", "", "", &options).is_err());
        }
        assert!(map("save_files", "", "", &Options::new()).is_err());
        let mut options = Options::new();
        put(
            &mut options,
            "files",
            Value::from(vec![b"..\0".to_vec(), b"/\0".to_vec()]),
        );
        assert!(map("save_files", "", "", &options).is_err());
    }

    #[test]
    fn lenient_lists_defaults_limits_and_standalone_filter() {
        let mut options = Options::new();
        put(&mut options, "accept_label", Value::from(""));
        put(
            &mut options,
            "current_folder",
            Value::from(b"relative\0".to_vec()),
        );
        put(&mut options, "current_name", Value::from("a/b"));
        put(&mut options, "filters", Value::from(Vec::<Filter>::new()));
        let mut patterns: Vec<(u32, String)> = vec![(0, "*.txt".to_owned()); 300];
        patterns.push((0, "x".repeat(257)));
        patterns.push((2, "unknown".into()));
        patterns.push((0, "[unclosed".into()));
        put(
            &mut options,
            "current_filter",
            Value::from(("é".repeat(100), patterns)),
        );
        let choice: Choice = ("id".into(), "Label".into(), vec![], "".into());
        put(
            &mut options,
            "choices",
            Value::from(vec![choice.clone(), choice]),
        );
        let body = map("save", "", &"é".repeat(200), &options).unwrap();
        for key in ["accept_label", "current_folder", "current_name"] {
            assert!(body.get(key).is_none());
        }
        assert_eq!(body["title"].as_str().unwrap().len(), 256);
        assert_eq!(body["filters"], json!([]));
        assert_eq!(body["current_filter"]["name"].as_str().unwrap().len(), 128);
        assert_eq!(
            body["current_filter"]["patterns"].as_array().unwrap().len(),
            300
        );
        assert_eq!(body["choices"].as_array().unwrap().len(), 1);
        assert!(body["choices"][0].get("default").is_none());
    }

    #[test]
    fn combined_native_body_budget_keeps_first_entries_without_inventing_fields() {
        let mut options = Options::new();
        let files: Vec<Vec<u8>> = (0..256).map(|i| {
            let mut bytes = vec![0xff; 250];
            bytes.extend(format!("{i:03}").as_bytes()); bytes.push(0); bytes
        }).collect();
        put(&mut options, "files", Value::from(files));
        let choices: Vec<Choice> = (0..16).map(|i| (format!("choice{i}"), "\"".repeat(128),
            (0..32).map(|n| (format!("option{n}"), "\"".repeat(128))).collect(), "".into())).collect();
        put(&mut options, "choices", Value::from(choices));
        let body = map("save_files", "", "", &options).unwrap();
        assert!(serde_json::to_vec(&body).unwrap().len() <= 250 * 1024);
        assert!(!body["files"].as_array().unwrap().is_empty());
        assert!(body.get("filters").is_none());
        assert!(body.get("current_filter").is_none());
    }
}
