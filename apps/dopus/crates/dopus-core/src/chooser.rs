// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Pure `files.choose` request engine (docs/docs/spec/files/README.md).
//!
//! The host supplies time, an incarnation, translated defaults and a presenter.
//! `Presenter::open` prepares a window with input disabled. Only after sending
//! the immediate Bus reply may the host call `Engine::reply_sent`. Presenter
//! events go through `Engine::end` on the same thread. Its returned outcome is
//! the sole publication: later ends return `None`. No file is opened or written.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MAX_BODY: usize = 256 * 1024;
const RETENTION_MS: u64 = 600_000;
const RATE_MS: u64 = 10_000;

/// All four commands; the bool denotes a query.
pub const COMMANDS: &[(&str, bool)] = &[
    ("files.choose", false),
    ("files.choose.result", true),
    ("files.choose.cancel", false),
    ("files.choose.list", true),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub error_code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

impl Refusal {
    pub fn new(error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
            reason: None,
        }
    }

    fn argument(message: impl Into<String>) -> Self {
        Self::new("ARGUMENT", message)
    }
}

type Result<T> = std::result::Result<T, Refusal>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Open,
    Save,
    SaveFiles,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<String>,
    pub mime_types: Vec<String>,
}

impl Filter {
    /// Supply the MIME type from the shared MIME database. Folders always pass.
    pub fn matches(&self, name: &str, directory: bool, mime: &str) -> bool {
        directory
            || self.patterns.iter().any(|pattern| {
                glob::Pattern::new(pattern).is_ok_and(|p| {
                    p.matches_with(
                        name,
                        glob::MatchOptions {
                            case_sensitive: true,
                            require_literal_separator: true,
                            require_literal_leading_dot: false,
                        },
                    )
                })
            })
            || self.mime_types.iter().any(|pattern| {
                pattern == mime
                    || pattern.strip_suffix("/*").is_some_and(|major| {
                        mime.strip_prefix(major)
                            .is_some_and(|tail| tail.starts_with('/'))
                    })
            })
    }

    /// The save-only `*.ext` rule; complex globs do not suggest an extension.
    pub fn save_name(&self, name: &[u8]) -> Vec<u8> {
        if self.patterns.len() == 1
            && let Some(ext) = self.patterns[0].strip_prefix("*.")
            && !ext.is_empty()
            && !ext.contains(['*', '?', '[', ']', '/'])
            && !name.iter().skip(1).any(|b| *b == b'.')
        {
            let mut name = name.to_vec();
            name.push(b'.');
            name.extend_from_slice(ext.as_bytes());
            return name;
        }
        name.to_vec()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OptionLabel {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub options: Vec<OptionLabel>,
    pub default: String,
}

/// Defaults belong to the host's Fluent catalogue, not this engine.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub open_title: String,
    pub save_title: String,
    pub save_files_title: String,
    pub open_accept: String,
    pub save_accept: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub mode: Mode,
    pub title: String,
    pub accept_label: String,
    pub multiple: bool,
    pub directory: bool,
    /// Menu order, with a nonmatching current filter inserted first.
    pub filters: Vec<Filter>,
    pub filter_menu: bool,
    pub current_filter: Option<Filter>,
    pub choices: Vec<Choice>,
    pub current_folder: Option<Vec<u8>>,
    pub current_name: Option<Vec<u8>>,
    pub current_file: Option<Vec<u8>>,
    pub files: Vec<Vec<u8>>,
    pub parent_window: String,
    pub correlation: Option<String>,
    pub on_behalf_of: Option<String>,
}

fn bounded(text: &str, min: usize, max: usize, field: &str) -> Result<()> {
    if (min..=max).contains(&text.len()) {
        Ok(())
    } else {
        Err(Refusal::argument(format!(
            "{field}: expected {min}..{max} bytes"
        )))
    }
}

fn object(body: &str) -> Result<Map<String, Value>> {
    if body.len() > MAX_BODY {
        return Err(Refusal::argument("body exceeds 256 KiB"));
    }
    let value: Value = serde_json::from_str(if body.trim().is_empty() { "{}" } else { body })
        .map_err(|_| Refusal::argument("expected a JSON object"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Refusal::argument("expected a JSON object"))
}

fn text<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<Option<&'a str>> {
    m.get(key)
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| Refusal::argument(format!("{key}: expected string")))?;
            bounded(s, min, max, key)?;
            Ok(s)
        })
        .transpose()
}

fn boolean(m: &Map<String, Value>, key: &str) -> Result<bool> {
    m.get(key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| Refusal::argument(format!("{key}: expected bool")))
        })
        .unwrap_or(Ok(false))
}

fn list<'a>(m: &'a Map<String, Value>, key: &str, min: usize, max: usize) -> Result<&'a [Value]> {
    let Some(v) = m.get(key) else {
        return Ok(&[]);
    };
    let items = v
        .as_array()
        .ok_or_else(|| Refusal::argument(format!("{key}: expected list")))?;
    if !(min..=max).contains(&items.len()) {
        return Err(Refusal::argument(format!(
            "{key}: expected {min}..{max} entries"
        )));
    }
    Ok(items)
}

/// Strict decoding, retaining exact bytes rather than replacing invalid UTF-8.
pub fn percent_decode(s: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut input = s.as_bytes().iter().copied();
    while let Some(b) = input.next() {
        if b == b'%' {
            let hi = input.next().and_then(|c| (c as char).to_digit(16));
            let lo = input.next().and_then(|c| (c as char).to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
                _ => return Err(Refusal::argument("invalid percent encoding")),
            }
        } else {
            bytes.push(b);
        }
    }
    Ok(bytes)
}

fn valid_path(path: &[u8]) -> Result<()> {
    if path.first() != Some(&b'/') || path.len() > 4096 || path.contains(&0) {
        Err(Refusal::argument(
            "location: expected absolute path, at most 4096 bytes, no NUL",
        ))
    } else {
        Ok(())
    }
}

pub fn location(value: &Value) -> Result<Vec<u8>> {
    let s = value
        .as_str()
        .ok_or_else(|| Refusal::argument("location: expected string"))?;
    let bytes = if s
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        let rest = &s[7..];
        let path = if rest.starts_with('/') {
            rest
        } else {
            let (host, _) = rest
                .split_once('/')
                .ok_or_else(|| Refusal::argument("file URI requires an absolute path"))?;
            if !host.eq_ignore_ascii_case("localhost") {
                return Err(Refusal::argument("file URI: expected no host or localhost"));
            }
            &rest[host.len()..]
        };
        if !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-._~%!$&'()*+,;=:@".contains(&b))
        {
            return Err(Refusal::argument(
                "file URI: invalid path character; percent-encode path bytes",
            ));
        }
        percent_decode(path)?
    } else {
        s.as_bytes().to_vec()
    };
    valid_path(&bytes)?;
    Ok(bytes)
}

pub fn validate_name(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty()
        || bytes.len() > 255
        || bytes.contains(&b'/')
        || bytes.contains(&0)
        || bytes == b"."
        || bytes == b".."
    {
        Err(Refusal::argument(
            "name: expected one component, 1..255 bytes, no slash or NUL, neither . nor ..",
        ))
    } else {
        Ok(())
    }
}

pub fn name(value: &Value) -> Result<Vec<u8>> {
    let bytes = if let Some(s) = value.as_str() {
        s.as_bytes().to_vec()
    } else if let Some(s) = value
        .as_object()
        .and_then(|m| m.get("bytes"))
        .and_then(Value::as_str)
    {
        percent_decode(s)?
    } else {
        return Err(Refusal::argument(
            "name: expected string or {bytes: string}",
        ));
    };
    validate_name(&bytes)?;
    Ok(bytes)
}

pub fn file_uri(path: &[u8]) -> Result<String> {
    valid_path(path)?;
    let mut uri = String::from("file://");
    use std::fmt::Write;
    for &b in path {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            write!(&mut uri, "%{b:02X}").expect("writing a string");
        }
    }
    Ok(uri)
}

fn parse_filter(value: &Value) -> Result<Filter> {
    let filter: Filter = serde_json::from_value(value.clone())
        .map_err(|_| Refusal::argument("filter: expected {name, patterns, mime_types}"))?;
    bounded(&filter.name, 1, 128, "filter.name")?;
    let entries = filter.patterns.len() + filter.mime_types.len();
    if !(1..=512).contains(&entries) {
        return Err(Refusal::argument("filter: expected 1..512 entries"));
    }
    for s in filter.patterns.iter().chain(&filter.mime_types) {
        bounded(s, 1, 256, "filter entry")?;
    }
    Ok(filter)
}

fn parse_choice(value: &Value) -> Result<Choice> {
    let m = value
        .as_object()
        .ok_or_else(|| Refusal::argument("choice: expected object"))?;
    let id = text(m, "id", 1, 64)?
        .ok_or_else(|| Refusal::argument("choice.id is required"))?
        .to_owned();
    let label = text(m, "label", 1, 128)?
        .ok_or_else(|| Refusal::argument("choice.label is required"))?
        .to_owned();
    if !m.contains_key("options") {
        return Err(Refusal::argument("choice.options is required"));
    }
    let mut options = Vec::new();
    let mut ids = HashSet::new();
    for value in list(m, "options", 0, 32)? {
        let option: OptionLabel = serde_json::from_value(value.clone())
            .map_err(|_| Refusal::argument("option: expected {id, label}"))?;
        bounded(&option.id, 1, 64, "option.id")?;
        bounded(&option.label, 1, 128, "option.label")?;
        if !ids.insert(option.id.clone()) {
            return Err(Refusal::argument("duplicate option id"));
        }
        options.push(option);
    }
    let default = text(m, "default", 1, 64)?
        .map(str::to_owned)
        .unwrap_or_else(|| {
            options
                .first()
                .map(|o| o.id.clone())
                .unwrap_or_else(|| "false".into())
        });
    if if options.is_empty() {
        default != "true" && default != "false"
    } else {
        !ids.contains(&default)
    } {
        return Err(Refusal::argument("invalid choice default"));
    }
    Ok(Choice {
        id,
        label,
        options,
        default,
    })
}

impl Request {
    pub fn parse(body: &str, defaults: &Defaults) -> Result<Self> {
        let m = object(body)?;
        let mode = match text(&m, "mode", 1, 10)?.unwrap_or("open") {
            "open" => Mode::Open,
            "save" => Mode::Save,
            "save_files" => Mode::SaveFiles,
            _ => return Err(Refusal::argument("mode: expected open, save or save_files")),
        };
        for key in [
            "multiple",
            "directory",
            "filters",
            "current_filter",
            "current_name",
            "current_file",
            "files",
        ] {
            let allowed = match key {
                "multiple" | "directory" => mode == Mode::Open,
                "filters" | "current_filter" => mode != Mode::SaveFiles,
                "files" => mode == Mode::SaveFiles,
                _ => mode == Mode::Save,
            };
            if m.contains_key(key) && !allowed {
                return Err(Refusal::argument(format!("{key}: wrong mode")));
            }
        }
        let title = text(&m, "title", 1, 256)?
            .unwrap_or(match mode {
                Mode::Open => &defaults.open_title,
                Mode::Save => &defaults.save_title,
                Mode::SaveFiles => &defaults.save_files_title,
            })
            .to_owned();
        let accept_label = text(&m, "accept_label", 1, 64)?
            .unwrap_or(if mode == Mode::Open {
                &defaults.open_accept
            } else {
                &defaults.save_accept
            })
            .replace('_', "");
        let mut filters: Vec<_> = list(&m, "filters", 0, 64)?
            .iter()
            .map(parse_filter)
            .collect::<Result<_>>()?;
        let filter_menu = !filters.is_empty();
        let current_filter = m
            .get("current_filter")
            .map(parse_filter)
            .transpose()?
            .or_else(|| filters.first().cloned());
        if filter_menu
            && let Some(current) = &current_filter
            && !filters.contains(current)
        {
            filters.insert(0, current.clone());
        }
        let choices: Vec<_> = list(&m, "choices", 0, 16)?
            .iter()
            .map(parse_choice)
            .collect::<Result<_>>()?;
        let mut ids = HashSet::new();
        for choice in &choices {
            if !ids.insert(&choice.id) {
                return Err(Refusal::argument("duplicate choice id"));
            }
        }
        let files: Vec<_> = list(&m, "files", 1, 256)?
            .iter()
            .map(name)
            .collect::<Result<_>>()?;
        if mode == Mode::SaveFiles && files.is_empty() {
            return Err(Refusal::argument("save_files requires files"));
        }
        let mut names = HashSet::new();
        for file in &files {
            if !names.insert(file) {
                return Err(Refusal::argument("duplicate files name"));
            }
        }
        // Validate even the fields that current_file overrides.
        let mut current_folder = m.get("current_folder").map(location).transpose()?;
        let mut current_name = m.get("current_name").map(name).transpose()?;
        let current_file = m.get("current_file").map(location).transpose()?;
        if let Some(file) = &current_file {
            let slash = file
                .iter()
                .rposition(|b| *b == b'/')
                .expect("absolute path");
            current_folder = Some(if slash == 0 {
                vec![b'/']
            } else {
                file[..slash].to_vec()
            });
            current_name = Some(file[slash + 1..].to_vec());
        }
        Ok(Self {
            mode,
            title,
            accept_label,
            multiple: boolean(&m, "multiple")?,
            directory: boolean(&m, "directory")?,
            filters,
            filter_menu,
            current_filter,
            choices,
            current_folder,
            current_name,
            current_file,
            files,
            parent_window: text(&m, "parent_window", 0, 256)?.unwrap_or("").to_owned(),
            correlation: text(&m, "correlation", 1, 256)?.map(str::to_owned),
            on_behalf_of: text(&m, "on_behalf_of", 1, 128)?.map(str::to_owned),
        })
    }

    pub fn choice_defaults(&self) -> BTreeMap<String, String> {
        self.choices
            .iter()
            .map(|c| (c.id.clone(), c.default.clone()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct OpenRequest {
    pub id: String,
    pub caller: String,
    pub opened_ms: u64,
    pub request: Request,
    ready: bool,
}

/// A failed open leaves no window or queued events. Input stays disabled until
/// `input_ready`. The host forwards later events to `Engine::end`.
pub trait Presenter {
    fn open(&mut self, request: &OpenRequest) -> std::result::Result<(), Unavailable>;
    fn input_ready(&mut self, request: &str);
    fn close(&mut self, request: &str);
}

#[derive(Clone, Debug)]
pub struct Unavailable(pub String);

#[derive(Clone, Debug)]
pub struct Acceptance {
    pub paths: Vec<Vec<u8>>,
    /// Required for save_files, whose paths must be exactly folder/name.
    pub folder: Option<Vec<u8>>,
    pub current_filter: Option<Filter>,
    /// Missing values take their request defaults.
    pub choices: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub enum End {
    Accept(Acceptance),
    Cancel,
    Caller,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub request: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<String>,
    pub response: u8,
    pub reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uris: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_filter: Option<Filter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<BTreeMap<String, String>>,
}

/// Caller limits use a tuple, so a claimed app cannot collide with a connection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Caller(String, Option<String>);

pub struct Engine {
    incarnation: String,
    sequence: u64,
    defaults: Defaults,
    home: Vec<u8>,
    open: BTreeMap<u64, OpenRequest>,
    done: VecDeque<(u64, Outcome)>,
    recent: HashMap<Caller, VecDeque<u64>>,
    remembered: HashMap<(bool, String), Vec<u8>>,
}

impl Engine {
    pub fn new(incarnation: u64, defaults: Defaults, home: Vec<u8>) -> Result<Self> {
        valid_path(&home)?;
        Ok(Self {
            incarnation: format!("{incarnation:016x}"),
            sequence: 0,
            defaults,
            home,
            open: BTreeMap::new(),
            done: VecDeque::new(),
            recent: HashMap::new(),
            remembered: HashMap::new(),
        })
    }

    pub fn sweep(&mut self, now: u64) {
        while self
            .done
            .front()
            .is_some_and(|(at, _)| now.saturating_sub(*at) >= RETENTION_MS)
        {
            self.done.pop_front();
        }
        self.recent.retain(|_, times| {
            while times
                .front()
                .is_some_and(|at| now.saturating_sub(*at) >= RATE_MS)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
    }

    /// Prepare a chooser. The returned reply must be sent before `reply_sent`.
    pub fn choose(
        &mut self,
        body: &str,
        caller: &str,
        now: u64,
        presenter: &mut impl Presenter,
    ) -> Result<Value> {
        let mut request = Request::parse(body, &self.defaults)?;
        self.sweep(now);
        let key = Caller(caller.to_owned(), request.on_behalf_of.clone());
        let recent = self.recent.entry(key.clone()).or_default();
        if recent.len() >= 10 {
            return Err(Refusal::new("BUSY", "chooser rate limit reached"));
        }
        // Count valid attempts, including window-limit and availability
        // refusals. These counters retain no request, window or outcome.
        recent.push_back(now);
        if self.open.len() >= 8
            || self
                .open
                .values()
                .filter(|o| Caller(o.caller.clone(), o.request.on_behalf_of.clone()) == key)
                .count()
                >= 2
        {
            return Err(Refusal::new("BUSY", "chooser limit reached"));
        }
        if request.current_folder.is_none() {
            request.current_folder = Some(
                self.remembered
                    .get(&remember_key(caller, &request))
                    .unwrap_or(&self.home)
                    .clone(),
            );
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Refusal::new("UNAVAILABLE", "request sequence exhausted"))?;
        let id = format!("{}-{sequence}", self.incarnation);
        let opened = OpenRequest {
            id: id.clone(),
            caller: caller.to_owned(),
            opened_ms: now,
            request,
            ready: false,
        };
        presenter
            .open(&opened)
            .map_err(|e| Refusal::new("UNAVAILABLE", e.0))?;
        self.sequence = sequence;
        let mut reply = json!({"request":id, "topic":"files.chosen"});
        if let Some(correlation) = &opened.request.correlation {
            reply["correlation"] = json!(correlation);
        }
        self.open.insert(sequence, opened);
        Ok(reply)
    }

    pub fn reply_sent(&mut self, id: &str, presenter: &mut impl Presenter) {
        if let Ok(Some(seq)) = self.own_sequence(id)
            && let Some(open) = self.open.get_mut(&seq)
            && !open.ready
        {
            open.ready = true;
            presenter.input_ready(id);
        }
    }

    /// Close a prepared window whose reply could not be sent. The host must
    /// then stop this incarnation: it cannot fulfil this accepted request.
    pub fn reply_failed(&mut self, id: &str, presenter: &mut impl Presenter) {
        if let Ok(Some(seq)) = self.own_sequence(id)
            && self.open.remove(&seq).is_some()
        {
            presenter.close(id);
        }
    }

    pub fn result(&mut self, body: &str, now: u64) -> Result<Value> {
        self.sweep(now);
        let id = request_id(body)?;
        let seq = self.own_sequence(&id)?;
        if seq.is_some_and(|s| self.open.contains_key(&s)) {
            return Ok(json!({"state":"open"}));
        }
        if let Some((_, outcome)) = self.done.iter().find(|(_, outcome)| outcome.request == id) {
            return Ok(json!({"state":"done", "outcome":outcome}));
        }
        let reason = if seq.is_some_and(|s| s <= self.sequence) {
            "expired"
        } else {
            "unknown"
        };
        Err(Refusal {
            reason: Some(reason),
            ..Refusal::argument(reason)
        })
    }

    pub fn cancel(
        &mut self,
        body: &str,
        now: u64,
        presenter: &mut impl Presenter,
    ) -> Result<Option<Outcome>> {
        let id = request_id(body)?;
        self.own_sequence(&id)?;
        self.end(&id, End::Caller, now, presenter)
    }

    pub fn list(&self) -> Value {
        json!({"incarnation":self.incarnation, "requests":self.open.values().map(|o| {
            json!({"request":o.id,"mode":o.request.mode,"title":o.request.title,"caller":o.caller,
                "on_behalf_of":o.request.on_behalf_of,"correlation":o.request.correlation,"opened_ms":o.opened_ms})
        }).collect::<Vec<_>>()})
    }

    /// First valid end wins. A rejected acceptance leaves the chooser open.
    pub fn end(
        &mut self,
        id: &str,
        end: End,
        now: u64,
        presenter: &mut impl Presenter,
    ) -> Result<Option<Outcome>> {
        self.sweep(now);
        let Some(seq) = self.own_sequence(id)? else {
            return Ok(None);
        };
        let Some(open) = self.open.get(&seq) else {
            return Ok(None);
        };
        if !open.ready {
            return Err(Refusal::new("BUSY", "choose reply has not been sent"));
        }
        let mut outcome = Outcome {
            request: id.to_owned(),
            correlation: open.request.correlation.clone(),
            response: 1,
            reason: "cancelled",
            message: None,
            uris: None,
            current_filter: None,
            choices: None,
        };
        match end {
            End::Accept(accept) => {
                let (uris, filter, choices, folder) = accepted(&open.request, accept)?;
                self.remembered
                    .insert(remember_key(&open.caller, &open.request), folder);
                outcome.response = 0;
                outcome.reason = "chosen";
                outcome.uris = Some(uris);
                outcome.current_filter = filter;
                outcome.choices = Some(choices);
            }
            End::Cancel => {}
            End::Caller => outcome.reason = "caller",
            End::Failed(message) => {
                outcome.response = 2;
                outcome.reason = "failed";
                outcome.message = Some(message);
            }
        }
        self.open.remove(&seq);
        presenter.close(id);
        self.done.push_back((now, outcome.clone()));
        while self.done.len() > 64 {
            self.done.pop_front();
        }
        Ok(Some(outcome))
    }

    fn own_sequence(&self, id: &str) -> Result<Option<u64>> {
        let (prefix, seq) = id
            .split_once('-')
            .ok_or_else(|| Refusal::argument("malformed request"))?;
        if prefix.len() != 16
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || seq.starts_with('0')
            || !seq.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Refusal::argument("malformed request"));
        }
        let seq: u64 = seq
            .parse()
            .map_err(|_| Refusal::argument("malformed request"))?;
        Ok((prefix == self.incarnation).then_some(seq))
    }
}

pub fn request_id(body: &str) -> Result<String> {
    text(&object(body)?, "request", 1, 37)?
        .map(str::to_owned)
        .ok_or_else(|| Refusal::argument("request is required"))
}

fn remember_key(caller: &str, request: &Request) -> (bool, String) {
    request
        .on_behalf_of
        .as_ref()
        .map(|app| (true, app.clone()))
        .unwrap_or_else(|| (false, caller.to_owned()))
}

type Accepted = (
    Vec<String>,
    Option<Filter>,
    BTreeMap<String, String>,
    Vec<u8>,
);

fn accepted(request: &Request, accept: Acceptance) -> Result<Accepted> {
    if accept.paths.is_empty()
        || (request.mode != Mode::SaveFiles
            && (request.mode == Mode::Save || !request.multiple)
            && accept.paths.len() != 1)
    {
        return Err(Refusal::argument("wrong number of chosen paths"));
    }
    for path in &accept.paths {
        valid_path(path)?;
    }
    let first = &accept.paths[0];
    let folder = if request.mode == Mode::SaveFiles {
        let folder = accept
            .folder
            .ok_or_else(|| Refusal::argument("save_files requires chosen folder"))?;
        valid_path(&folder)?;
        let mut expected = Vec::new();
        for name in &request.files {
            let mut path = folder.clone();
            if path.last() != Some(&b'/') {
                path.push(b'/');
            }
            path.extend_from_slice(name);
            valid_path(&path)?;
            expected.push(path);
        }
        if accept.paths != expected {
            return Err(Refusal::argument(
                "save_files paths must be exactly folder/name, in request order",
            ));
        }
        folder
    } else if request.directory {
        first.clone()
    } else {
        let slash = first
            .iter()
            .rposition(|b| *b == b'/')
            .expect("absolute path");
        if slash == 0 {
            vec![b'/']
        } else {
            first[..slash].to_vec()
        }
    };
    let filter = accept
        .current_filter
        .or_else(|| request.current_filter.clone());
    if let Some(filter) = &filter {
        if !request.filters.contains(filter) && request.current_filter.as_ref() != Some(filter) {
            return Err(Refusal::argument("chosen filter was not offered"));
        }
    } else if request.current_filter.is_some() {
        return Err(Refusal::argument("chosen filter is required"));
    }
    let mut choices = request.choice_defaults();
    for (id, value) in accept.choices {
        let choice = request
            .choices
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Refusal::argument("unknown chosen choice"))?;
        if if choice.options.is_empty() {
            value != "true" && value != "false"
        } else {
            !choice.options.iter().any(|o| o.id == value)
        } {
            return Err(Refusal::argument("invalid chosen choice"));
        }
        choices.insert(id, value);
    }
    Ok((
        accept
            .paths
            .iter()
            .map(|p| file_uri(p))
            .collect::<Result<_>>()?,
        filter,
        choices,
        folder,
    ))
}

#[cfg(test)]
mod tests;
