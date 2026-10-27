// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pull-only, process-local diagnostics. No allocation while disabled.
use crate::debug::Level;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::Context};

const MAX_EVENTS: usize = 2_000;
const MAX_BYTES: usize = 1_000_000;
const MAX_TEXT: usize = 4_096;
const MAX_FIELDS: usize = 32;
static ENABLED: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static RING: Mutex<Option<Box<Ring>>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn allocated() -> bool {
    RING.lock().unwrap().is_some()
}

#[derive(Clone, Serialize)]
struct Entry {
    seq: u64,
    ts: String,
    level: Level,
    target: String,
    message: String,
    fields: Map<String, Value>,
}

struct Ring {
    entries: VecDeque<(Entry, usize)>,
    bytes: usize,
}

impl Ring {
    fn new() -> Self {
        let entries = VecDeque::new();
        let bytes = std::mem::size_of::<Self>();
        Self { entries, bytes }
    }

    fn push(&mut self, entry: Entry) {
        // Conservative resident-size charge includes map nodes, JSON values,
        // deque slots and allocator overhead, rather than just wire bytes.
        let bytes = 256
            + entry.ts.len()
            + entry.target.len()
            + entry.message.len()
            + entry
                .fields
                .iter()
                .map(|(k, v)| k.len() + v.as_str().map_or(32, str::len) + 256)
                .sum::<usize>();
        while self.entries.len() >= MAX_EVENTS || self.bytes + bytes > MAX_BYTES {
            match self.entries.pop_front() {
                Some((_, old)) => self.bytes -= old,
                None => break,
            }
        }
        if self.bytes + bytes <= MAX_BYTES {
            self.bytes += bytes;
            let capacity = self.entries.capacity();
            self.entries.push_back((entry, bytes));
            // Charge storage only when the deque actually grows. Capacity
            // survives eviction, so keep its charge until the ring is freed.
            self.bytes +=
                (self.entries.capacity() - capacity) * std::mem::size_of::<(Entry, usize)>();
            while self.bytes > MAX_BYTES {
                if let Some((_, old)) = self.entries.pop_front() {
                    self.bytes -= old;
                } else {
                    break;
                }
            }
        }
    }
}

pub(crate) fn configure(
    filter: &tracing_subscriber::EnvFilter,
    baseline: &tracing_subscriber::EnvFilter,
) {
    let verbosity = |filter: &tracing_subscriber::EnvFilter| {
        filter
            .max_level_hint()
            .unwrap_or(tracing::level_filters::LevelFilter::TRACE)
    };
    // Baselines may already include info, debug or trace directives. Retain
    // diagnostics only while the live verbosity exceeds that recorded level.
    let level = verbosity(filter);
    let enabled = level > tracing::level_filters::LevelFilter::WARN && level > verbosity(baseline);
    let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    if enabled {
        ring.get_or_insert_with(|| Box::new(Ring::new()));
    } else {
        *ring = None;
    }
    ENABLED.store(enabled, Ordering::Release);
}

/// Request for `<app>.debug.recent` and `comp.debug.recent`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentRequest {
    pub since: Option<u64>,
    pub level: Option<Level>,
    pub limit: Option<usize>,
}

pub(crate) fn query(request: RecentRequest) -> Result<Value, String> {
    let limit = request.limit.unwrap_or(200);
    if !(1..=1_000).contains(&limit) {
        return Err("limit must be between 1 and 1000".into());
    }
    let ring = RING.lock().map_err(|e| e.to_string())?;
    let seq = SEQ.load(Ordering::Relaxed);
    let events = ring
        .as_ref()
        .map(|ring| {
            let matching = || {
                ring.entries.iter().map(|(entry, _)| entry).filter(|entry| {
                    request.since.is_none_or(|since| entry.seq > since)
                        && request.level.is_none_or(|level| entry.level <= level)
                })
            };
            // Without a cursor return the most recent page; with a cursor walk
            // forward, so a capped page cannot skip entries on the next pull.
            let skip = if request.since.is_none() {
                matching().count().saturating_sub(limit)
            } else {
                0
            };
            matching()
                .skip(skip)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let next = events
        .last()
        .map_or(request.since.unwrap_or(seq), |entry| entry.seq);
    Ok(json!({"events": events, "seq": seq, "next": next, "recording": ring.is_some()}))
}

pub(crate) struct RecentLayer;
impl<S: tracing::Subscriber> Layer<S> for RecentLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if !ENABLED.load(Ordering::Acquire) {
            return;
        }
        let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ring) = ring.as_mut() else {
            return;
        };
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        ring.push(Entry {
            seq: SEQ.fetch_add(1, Ordering::Relaxed) + 1,
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            level: match *metadata.level() {
                tracing::Level::ERROR => Level::Error,
                tracing::Level::WARN => Level::Warn,
                tracing::Level::INFO => Level::Info,
                tracing::Level::DEBUG => Level::Debug,
                tracing::Level::TRACE => Level::Trace,
            },
            target: bounded(metadata.target()),
            message: visitor.message,
            fields: visitor.values,
        });
    }
}

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[derive(Default)]
struct Text(String);
impl Write for Text {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let mut end = text.len().min(MAX_TEXT.saturating_sub(self.0.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push_str(&text[..end]);
        Ok(())
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    values: Map<String, Value>,
}
impl Fields {
    fn insert(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = value.as_str().unwrap_or_default().to_owned();
        } else if self.values.len() < MAX_FIELDS {
            self.values.insert(bounded(field.name()), value);
        }
    }
}
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let mut text = Text::default();
        let _ = write!(text, "{value:?}");
        self.insert(field, Value::String(text.0));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(bounded(value)));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, value.into());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, json!(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_baseline_allocates_only_while_raised() {
        const CHILD: &str = "MIXOS_RECENT_BASELINE_TEST";
        let Ok(flag) = std::env::var(CHILD) else {
            for flag in ["baseline", "--debug", "--debug=info", "--debug=warn"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "recent::tests::info_baseline_allocates_only_while_raised",
                    ])
                    .env(CHILD, flag)
                    .env_remove("RUST_LOG")
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{flag}: {output:?}");
            }
            return;
        };
        let mut args = vec!["fixture".into()];
        if flag != "baseline" {
            args.push(flag.clone());
        }
        let _guard = crate::debug::init("fixture", "warn,fixture=info", &mut args).unwrap();
        assert_eq!(allocated(), flag == "--debug");
        let initial = query(RecentRequest::default()).unwrap();
        assert_eq!(initial["recording"], flag == "--debug");
        if flag == "baseline" {
            tracing::info!(target: "fixture", "baseline startup");
            assert_eq!(query(RecentRequest::default()).unwrap(), initial);
        }
        assert_eq!(
            crate::debug::request("fixture", "fixture.debug.set", r#"{"level":"debug"}"#)
                .unwrap()
                .0,
            0
        );
        assert!(allocated());
        let seq = query(RecentRequest::default()).unwrap()["seq"].clone();
        assert_eq!(
            crate::debug::request("fixture", "fixture.debug.set", r#"{"on":false}"#)
                .unwrap()
                .0,
            0
        );
        assert!(!allocated());
        let stopped = query(RecentRequest::default()).unwrap();
        assert_eq!(stopped["recording"], false);
        assert_eq!(stopped["events"], json!([]));
        assert_eq!(stopped["seq"], seq);
    }

    fn entry(seq: u64, message: String) -> Entry {
        Entry {
            seq,
            ts: String::new(),
            level: Level::Debug,
            target: "logging".into(),
            message,
            fields: Map::new(),
        }
    }
    #[test]
    fn count_and_byte_bounds_evict_oldest() {
        let mut ring = Ring::new();
        assert_eq!(ring.entries.capacity(), 0);
        for seq in 1..=2_100 {
            ring.push(entry(seq, String::new()));
        }
        assert_eq!(ring.entries.len(), MAX_EVENTS);
        assert_eq!(ring.entries.front().unwrap().0.seq, 101);
        for seq in 2_101..=3_000 {
            ring.push(entry(seq, "x".repeat(MAX_TEXT)));
        }
        assert!(ring.bytes <= MAX_BYTES);
        assert!(ring.entries.len() < MAX_EVENTS);
        assert_eq!(ring.entries.back().unwrap().0.seq, 3_000);
    }
    #[test]
    fn growing_deque_storage_is_charged_against_the_byte_bound() {
        let mut ring = Ring::new();
        for seq in 1..=3_000 {
            ring.push(entry(seq, "x".repeat(650)));
            let storage = std::mem::size_of::<Ring>()
                + ring.entries.capacity() * std::mem::size_of::<(Entry, usize)>();
            let retained: usize = ring.entries.iter().map(|(_, bytes)| bytes).sum();
            assert_eq!(ring.bytes, storage + retained);
            assert!(ring.bytes <= MAX_BYTES);
        }
    }
    #[test]
    fn event_text_is_bounded_on_utf8_boundaries() {
        let mut text = Text::default();
        write!(text, "{}", "🦀".repeat(10_000)).unwrap();
        assert_eq!(text.0.len(), MAX_TEXT);
    }
}
