// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Wire adapter for the `<svc>.stats.snapshot` Bus command (plan §4.2).
//!
//! This is the `bus-handlers`-gated half of the command — it parses the
//! Bus headers documented in plan §4.2, calls [`snapshot_dispatch`] (the
//! Bus-agnostic core in `stats/dispatch.rs`), and serializes the
//! resulting [`Snapshot`] into the §4.2 response envelope. The 1 MiB
//! response cap lives here too — `dispatch` returns a structured
//! [`Snapshot`], not bytes, so the cap can only be checked after
//! serialization.
//!
//! # Why a free function and not a struct
//!
//! Slice 5b's "option 1" shape (see
//! `_doc/planned/logging-stats.md` §4.2 — *"This command is
//! registered on the host's existing `PropsRouter`"*) was deferred in
//! favour of matching every other custom command in the tree: a free
//! function the daemon's existing `cmd.command.strip_prefix(...)`
//! dispatch loop calls directly. `PropsRouter` is exclusively the
//! SPEC-12 `<svc>.props.*` router (see
//! `mixos-lib-props-store/src/bus/mutation.rs`); routing a non-props command
//! through it would have crossed an architectural boundary. The
//! free-function shape is the smallest blast radius and matches the
//! `maild::bus::{dkim,rules,bayesian,tls}::dispatch` precedent
//! verbatim. See the *project_p5_slice5b_open_question* memo for the
//! three options surfaced to the operator; option 1 (this one) was
//! the chosen path.
//!
//! # Wire shape (plan §4.2, frozen)
//!
//! Action: `<svc>.stats.snapshot`
//!
//! | Header        | Form                                  | Default |
//! |---------------|---------------------------------------|---------|
//! | `metric`      | `name`, `name*`, or `*name` glob      | none    |
//! | `labels`      | JSON object — scalar = exact-match,   |         |
//! |               | null = key-presence-only              | none    |
//! | `labels_hash` | comma-separated 16-hex FxHash digests | none    |
//!
//! Body: empty.
//!
//! Response body on success: `{ "service": ..., "captured_at": ...,
//! "metrics": [...] }` per plan §4.2; rc = [`RC_SUCCESS`].
//!
//! Errors report `ParseError` or `TooLarge`. The response remains bounded to 1 MiB.
//!
//! The byte cap is checked after building and serialising the snapshot.
//! Allocation before that check is unbounded; metric and label filters reduce
//! it, but the 1 MiB wire limit does not bound peak memory use.

use std::collections::BTreeMap;

use bus::native_client::IncomingCommand;
use serde_json::{Map as JsonMap, Value as JsonValue, json};

use crate::stats::dispatch::{LabelFilter, MetricPattern, SnapshotRequest, snapshot_dispatch};
use crate::stats::recorder::StatsRecorder;
use crate::stats::types::{
    HistogramSummary, MetricFamily, MetricKind, Series, SeriesLabels, SeriesValue, Snapshot,
};

/// Maximum serialized response size in bytes (plan §4.2). A response
/// that exceeds this is rejected with the `TooLarge` error envelope so
/// the broker doesn't ship a >1 MiB frame; operators add `metric` /
/// `labels` filters to narrow the response.
pub const SNAPSHOT_MAX_RESPONSE_BYTES: usize = 1_048_576;

/// rc=0 success sentinel (mirrors `bus::RC_SUCCESS` —
/// duplicated here to keep this module's RC contract local and avoid
/// a cross-crate dep for one constant).
const RC_OK: u8 = 0;
/// rc=10 caller-error sentinel; matches `NodedClient::call`'s
/// `rc >= 10 → Err` convention used by every other `maild.*` dispatch
/// helper in the tree (see `mixos-maild/src/bus/mod.rs:64`).
const RC_ERR: u8 = 10;

/// Serve every caller, retaining filter validation and the response byte bound.
pub fn handle_snapshot_bus(cmd: &IncomingCommand, recorder: &StatsRecorder) -> (u8, String) {
    let request = match parse_request(cmd) {
        Ok(request) => request,
        Err(reason) => return parse_error(&reason),
    };
    let snapshot = snapshot_dispatch(recorder, &request);

    // 5. Serialize to the plan §4.2 JSON shape.
    let body = serialize_snapshot(&snapshot);

    // 6. 1 MiB cap. We serialize first because the cap is on the
    //    wire bytes, and a structured Snapshot doesn't have a
    //    byte-length until rendered. Plan §4.2: "over-cap returns
    //    Err::TooLarge with an explanatory message naming the
    //    filter to add."
    if body.len() > SNAPSHOT_MAX_RESPONSE_BYTES {
        return too_large(body.len());
    }

    (RC_OK, body)
}

// ── Header parsing ────────────────────────────────────────────────

fn parse_request(cmd: &IncomingCommand) -> Result<SnapshotRequest, String> {
    let metric = match cmd.header("metric") {
        Some(s) if !s.is_empty() => Some(parse_metric_glob(s)?),
        _ => None,
    };
    let labels = match cmd.header("labels") {
        Some(s) if !s.is_empty() => parse_labels_filter(s)?,
        _ => Vec::new(),
    };
    let labels_hash = match cmd.header("labels_hash") {
        Some(s) if !s.is_empty() => parse_labels_hash(s)?,
        _ => Vec::new(),
    };
    Ok(SnapshotRequest {
        metric,
        labels,
        labels_hash,
    })
}

/// Parse the `metric` header glob per plan §4.2:
/// - `foo` → [`MetricPattern::Exact`]
/// - `foo*` → [`MetricPattern::Prefix`]
/// - `*foo` → [`MetricPattern::Suffix`]
///
/// No middle-`*` form, no escape sequence — the dispatch matcher is
/// deliberately trivial.
fn parse_metric_glob(s: &str) -> Result<MetricPattern, String> {
    let star_count = s.bytes().filter(|&b| b == b'*').count();
    match star_count {
        0 => Ok(MetricPattern::Exact(s.to_string())),
        1 => {
            if let Some(prefix) = s.strip_suffix('*') {
                if prefix.is_empty() {
                    return Err(
                        "metric `*` matches every family — omit the header instead".to_string()
                    );
                }
                Ok(MetricPattern::Prefix(prefix.to_string()))
            } else if let Some(suffix) = s.strip_prefix('*') {
                // The is_empty case is already covered by the
                // strip_suffix branch above (a bare `*` strips to ""
                // on both sides), but check defensively.
                if suffix.is_empty() {
                    return Err(
                        "metric `*` matches every family — omit the header instead".to_string()
                    );
                }
                Ok(MetricPattern::Suffix(suffix.to_string()))
            } else {
                Err(format!(
                    "metric {s:?} — single `*` must be at start (`*suffix`) or end (`prefix*`)"
                ))
            }
        }
        _ => Err(format!(
            "metric {s:?} — only one `*` is allowed (forms: `exact`, `prefix*`, `*suffix`)"
        )),
    }
}

/// Parse the `labels` header per plan §4.2 — a JSON object that
/// matches the SPEC-12 `props.list` filter shape (scalar value =
/// exact match) with the JSON `null` extension for "key must be
/// present, value unconstrained".
///
/// Supported entries:
/// - `"key": "value"` → [`LabelFilter::KeyEquals`]
/// - `"key": null` → [`LabelFilter::HasKey`]
///
/// Number / boolean / array / object entries are rejected — label
/// values in the recorder registry are always strings, so a non-string
/// scalar can't match. JSON booleans / numbers are explicitly called
/// out in the error so a caller writing JS-style filters
/// (`{"enabled": true}`) gets a clear correction.
///
/// **Duplicate keys: last-write-wins (`serde_json::Map` default).**
/// `{"user":"alice","user":null}` parses as `HasKey("user")` — the
/// `null` (second occurrence) overwrites the earlier `"alice"`. This
/// follows serde_json's last-write-wins object parsing.
fn parse_labels_filter(s: &str) -> Result<Vec<LabelFilter>, String> {
    let parsed: JsonMap<String, JsonValue> = serde_json::from_str(s)
        .map_err(|e| format!("labels must be a JSON object (e.g. {{\"verdict\":\"ham\"}}): {e}"))?;
    let mut filters = Vec::with_capacity(parsed.len());
    for (key, value) in parsed {
        match value {
            JsonValue::Null => filters.push(LabelFilter::HasKey(key)),
            JsonValue::String(v) => filters.push(LabelFilter::KeyEquals { key, value: v }),
            other => {
                return Err(format!(
                    "labels[{key:?}] = {other} — only string (exact match) or null (key-presence) supported; \
                     label values are strings in the recorder registry"
                ));
            }
        }
    }
    Ok(filters)
}

/// Parse the `labels_hash` header per plan §4.2 — a
/// comma-separated set of 16-hex-char FxHash digests.
///
/// Empty tokens are skipped (trailing-comma tolerant). Non-hex /
/// wrong-length tokens are rejected with a citation so the operator
/// can fix the typo.
fn parse_labels_hash(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for token in s.split(',') {
        let h = token.trim();
        if h.is_empty() {
            continue;
        }
        if h.len() != 16 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "labels_hash token {h:?} — expected 16-hex-char FxHash digest \
                 (matches the labels_hash field in JSONL stats records)"
            ));
        }
        out.push(h.to_ascii_lowercase());
    }
    Ok(out)
}

// ── Response serialization ────────────────────────────────────────

fn serialize_snapshot(snap: &Snapshot) -> String {
    let metrics: Vec<JsonValue> = snap.metrics.iter().map(family_to_json).collect();
    json!({
        "service": snap.service,
        "captured_at": snap
            .captured_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "metrics": metrics,
    })
    .to_string()
}

fn family_to_json(family: &MetricFamily) -> JsonValue {
    let mut obj = JsonMap::new();
    obj.insert("name".to_string(), JsonValue::String(family.name.clone()));
    obj.insert(
        "kind".to_string(),
        JsonValue::String(metric_kind_str(family.kind).to_string()),
    );
    if let Some(desc) = family.description.as_ref() {
        obj.insert("description".to_string(), JsonValue::String(desc.clone()));
    }
    obj.insert(
        "series".to_string(),
        JsonValue::Array(family.series.iter().map(series_to_json).collect()),
    );
    JsonValue::Object(obj)
}

fn metric_kind_str(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::Counter => "counter",
        MetricKind::Gauge => "gauge",
        MetricKind::Histogram => "histogram",
    }
}

fn series_to_json(series: &Series) -> JsonValue {
    let mut obj = JsonMap::new();
    match &series.labels {
        SeriesLabels::Raw(map) => {
            obj.insert("labels".to_string(), btree_to_json(map));
        }
        SeriesLabels::Hash(h) => {
            obj.insert("labels_hash".to_string(), JsonValue::String(h.clone()));
        }
    }
    obj.insert("value".to_string(), series_value_to_json(&series.value));
    JsonValue::Object(obj)
}

fn btree_to_json(map: &BTreeMap<String, String>) -> JsonValue {
    let mut obj = JsonMap::with_capacity(map.len());
    for (k, v) in map {
        obj.insert(k.clone(), JsonValue::String(v.clone()));
    }
    JsonValue::Object(obj)
}

fn series_value_to_json(value: &SeriesValue) -> JsonValue {
    match value {
        SeriesValue::Counter(n) => json!(n),
        SeriesValue::Gauge(f) => json!(f),
        SeriesValue::Histogram(h) => histogram_to_json(h),
    }
}

fn histogram_to_json(h: &HistogramSummary) -> JsonValue {
    json!({
        "count": h.count,
        "sum": h.sum,
        "p50": h.p50,
        "p95": h.p95,
        "p99": h.p99,
    })
}

// ── Error envelopes (plan §4.2) ───────────────────────────────────

fn parse_error(reason: &str) -> (u8, String) {
    (
        RC_ERR,
        json!({
            "error": "ParseError",
            "reason": reason,
        })
        .to_string(),
    )
}

fn too_large(actual: usize) -> (u8, String) {
    (
        RC_ERR,
        json!({
            "error": "TooLarge",
            "limit_bytes": SNAPSHOT_MAX_RESPONSE_BYTES,
            "actual_bytes": actual,
            "reason":
                "response exceeds 1 MiB cap — add a metric or labels filter to narrow the response",
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::classify::classify;
    use crate::stats::recorder::StatsRecorderBuilder;
    use crate::stats::types::LabelSensitivity;
    use bus::native_client::IncomingCommand;
    use metrics::{Counter, Key, Label, Recorder, SharedString};
    use serde_json::Value as JsonValue;
    use std::collections::BTreeMap;

    /// Build a recorder, register one counter family, and return both
    /// the recorder and a request stub. Mirrors the `build_with_counter`
    /// helper in `dispatch.rs` tests to keep the two suites symmetric.
    fn build_with_counter(
        service: &str,
        name: &'static str,
        labels: &[(&'static str, &'static str)],
        n: u64,
    ) -> StatsRecorder {
        let recorder = StatsRecorderBuilder::new(service).build();
        let label_vec: Vec<Label> = labels
            .iter()
            .map(|(k, v)| Label::from_static_parts(k, v))
            .collect();
        let key = Key::from_parts(SharedString::const_str(name), label_vec);
        let counter: Counter = recorder.register_counter(
            &key,
            &metrics::Metadata::new(name, metrics::Level::INFO, None),
        );
        counter.increment(n);
        recorder
    }

    fn make_cmd(headers: &[(&str, &str)]) -> IncomingCommand {
        let mut map = BTreeMap::new();
        for (k, v) in headers {
            map.insert((*k).to_string(), (*v).to_string());
        }
        IncomingCommand {
            generation: 0,
            from: String::new(),
            command: "maild.stats.snapshot".to_string(),
            id: None,
            args: JsonValue::Null,
            body: String::new(),
            headers: map,
        }
    }

    fn parse_body(body: &str) -> JsonValue {
        serde_json::from_str(body).expect("response body must be valid JSON")
    }

    // ── Cap admission ──────────────────────────────────────────────

    // ── metric? parsing ────────────────────────────────────────────

    #[test]
    fn metric_exact_match() {
        classify("bus_metric_exact_metric", LabelSensitivity::Safe);
        let recorder = build_with_counter(
            "bus-test",
            "bus_metric_exact_metric",
            &[("verdict", "ham")],
            3,
        );
        let cmd = make_cmd(&[("metric", "bus_metric_exact_metric")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let metrics = v["metrics"].as_array().expect("metrics array");
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0]["name"], "bus_metric_exact_metric");
        assert_eq!(metrics[0]["kind"], "counter");
        let series = metrics[0]["series"].as_array().expect("series array");
        assert_eq!(series[0]["value"], 3);
        assert_eq!(series[0]["labels"]["verdict"], "ham");
    }

    #[test]
    fn metric_prefix_match() {
        classify("bus_prefix_match_metric", LabelSensitivity::Safe);
        let recorder = build_with_counter("bus-test", "bus_prefix_match_metric", &[], 1);
        let cmd = make_cmd(&[("metric", "bus_prefix_*")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let metrics = v["metrics"].as_array().expect("metrics array");
        assert!(
            metrics
                .iter()
                .any(|m| m["name"] == "bus_prefix_match_metric")
        );
    }

    #[test]
    fn metric_suffix_match() {
        classify("bus_suffix_match_total", LabelSensitivity::Safe);
        let recorder = build_with_counter("bus-test", "bus_suffix_match_total", &[], 1);
        let cmd = make_cmd(&[("metric", "*_total")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let metrics = v["metrics"].as_array().expect("metrics array");
        assert!(
            metrics
                .iter()
                .any(|m| m["name"] == "bus_suffix_match_total")
        );
    }

    #[test]
    fn metric_two_stars_rejected() {
        let recorder = build_with_counter("bus-test", "bus_metric_two_stars_metric", &[], 1);
        // Two literal stars: the trailing `*` would look prefix-shaped
        // in isolation, but the middle `*` makes the whole pattern
        // ambiguous. The parser rejects on count, not position.
        let cmd = make_cmd(&[("metric", "bus_**metric")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
        let reason = v["reason"].as_str().expect("reason string");
        assert!(
            reason.contains("only one `*`"),
            "reason cites two-star shape: {reason}"
        );
    }

    #[test]
    fn metric_middle_star_rejected() {
        let recorder = build_with_counter("bus-test", "bus_middle_star_metric", &[], 1);
        let cmd = make_cmd(&[("metric", "foo*bar")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
    }

    #[test]
    fn metric_bare_star_rejected() {
        let recorder = build_with_counter("bus-test", "bus_bare_star_metric", &[], 1);
        let cmd = make_cmd(&[("metric", "*")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
        let reason = v["reason"].as_str().expect("reason string");
        assert!(
            reason.contains("matches every family"),
            "reason explains why bare `*` is rejected: {reason}"
        );
    }

    // ── labels? parsing ────────────────────────────────────────────

    #[test]
    fn labels_filter_key_equals_for_anonymous_caller() {
        // Value predicates and raw labels are available to every caller.
        let recorder = build_with_counter(
            "bus-test",
            "bus_labels_keyequals_metric",
            &[("user", "alice")],
            1,
        );
        let cmd = make_cmd(&[("labels", r#"{"user":"alice"}"#)]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        assert!(
            v["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["name"] == "bus_labels_keyequals_metric")
        );
    }

    #[test]
    fn labels_filter_haskey_returns_raw_labels() {
        // Key-presence predicates preserve raw labels.
        let recorder = build_with_counter(
            "bus-test",
            "bus_labels_haskey_metric",
            &[("region", "us"), ("user", "alice")],
            1,
        );
        let cmd = make_cmd(&[("labels", r#"{"region":null}"#)]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let family = v["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == "bus_labels_haskey_metric")
            .expect("family present");
        let series = family["series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        // Bus snapshots carry raw labels for every caller.
        assert!(series[0].get("labels").is_some());
        assert!(series[0].get("labels_hash").is_none());
    }

    #[test]
    fn labels_filter_non_object_rejected() {
        let recorder = build_with_counter("bus-test", "bus_labels_nonobject_metric", &[], 1);
        let cmd = make_cmd(&[("labels", "not-json")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
    }

    #[test]
    fn labels_filter_duplicate_keys_last_wins() {
        // Duplicate JSON keys follow serde_json's last-write-wins parsing.
        let recorder = build_with_counter(
            "bus-test",
            "bus_labels_dup_keys_metric",
            &[("region", "us")],
            1,
        );
        let cmd = make_cmd(&[("labels", r#"{"region":"us","region":null}"#)]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let family = v["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|metric| metric["name"] == "bus_labels_dup_keys_metric")
            .unwrap();
        assert_eq!(family["series"][0]["labels"]["region"], "us");
    }

    #[test]
    fn labels_filter_non_string_value_rejected() {
        let recorder = build_with_counter("bus-test", "bus_labels_number_metric", &[], 1);
        let cmd = make_cmd(&[("labels", r#"{"region":42}"#)]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        assert_eq!(parse_body(&body)["error"], "ParseError");
    }

    // ── labels_hash? parsing ───────────────────────────────────────

    #[test]
    fn labels_hash_filter_csv_matches() {
        // Build a Safe family so labels round-trip in the response
        // (the test asserts the labels_hash filter is consulted, not
        // the projection).
        classify("bus_labels_hash_csv_metric", LabelSensitivity::Safe);
        let recorder = build_with_counter(
            "bus-test",
            "bus_labels_hash_csv_metric",
            &[("verdict", "spam")],
            1,
        );

        // Compute the expected digest the way the dispatch does.
        let mut map = BTreeMap::new();
        map.insert("verdict".to_string(), "spam".to_string());
        let expected = crate::stats::labels_hash::labels_hash(&map);
        // Pad with a never-matching digest to exercise the CSV path
        // (multi-token list).
        let header = format!("{expected},0000000000000000");

        let cmd = make_cmd(&[("labels_hash", header.as_str())]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        assert!(
            v["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["name"] == "bus_labels_hash_csv_metric")
        );
    }

    #[test]
    fn labels_hash_filter_trailing_comma_ok() {
        classify("bus_labels_hash_trailing_metric", LabelSensitivity::Safe);
        let recorder = build_with_counter(
            "bus-test",
            "bus_labels_hash_trailing_metric",
            &[("verdict", "ham")],
            1,
        );
        let mut map = BTreeMap::new();
        map.insert("verdict".to_string(), "ham".to_string());
        let expected = crate::stats::labels_hash::labels_hash(&map);
        let header = format!("{expected},  ,");
        let cmd = make_cmd(&[("labels_hash", header.as_str())]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        assert!(
            v["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["name"] == "bus_labels_hash_trailing_metric")
        );
    }

    #[test]
    fn labels_hash_filter_wrong_length_rejected() {
        let recorder = build_with_counter("bus-test", "bus_labels_hash_short_metric", &[], 1);
        let cmd = make_cmd(&[("labels_hash", "deadbeef")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
    }

    #[test]
    fn labels_hash_filter_non_hex_rejected() {
        let recorder = build_with_counter("bus-test", "bus_labels_hash_nonhex_metric", &[], 1);
        let cmd = make_cmd(&[("labels_hash", "zzzzzzzzzzzzzzzz")]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
    }

    // ── Response envelope & projection ────────────────────────────

    #[test]
    fn safe_family_emits_raw_labels() {
        classify("bus_safe_passthrough_metric", LabelSensitivity::Safe);
        let recorder = build_with_counter(
            "bus-test",
            "bus_safe_passthrough_metric",
            &[("verdict", "ham")],
            1,
        );
        let cmd = make_cmd(&[]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let family = v["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == "bus_safe_passthrough_metric")
            .expect("family present");
        let series0 = &family["series"][0];
        assert!(series0.get("labels_hash").is_none());
        assert_eq!(series0["labels"]["verdict"], "ham");
    }

    #[test]
    fn captured_at_is_rfc3339_with_millis_z() {
        let recorder = build_with_counter("bus-test", "bus_captured_at_metric", &[], 1);
        let cmd = make_cmd(&[]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v = parse_body(&body);
        let ts = v["captured_at"].as_str().expect("captured_at string");
        // Shape: YYYY-MM-DDTHH:MM:SS.sssZ — Z-suffix (UTC), .sss millis.
        assert!(
            ts.ends_with('Z'),
            "captured_at must be Z-suffixed UTC: {ts}"
        );
        assert!(
            ts.contains('.'),
            "captured_at must carry millisecond fraction: {ts}"
        );
    }

    #[test]
    fn response_is_valid_json() {
        // Anti-regression: a future serializer that, say, double-quotes
        // a non-string field would still satisfy the JsonValue parser
        // but break downstream tooling. This test is a structural sanity
        // check, kept tiny so the cost stays low.
        let recorder = build_with_counter("bus-test", "bus_valid_json_metric", &[], 1);
        let cmd = make_cmd(&[]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_OK);
        let v: JsonValue = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["service"], "bus-test");
        assert!(v["metrics"].is_array());
    }

    // ── 1 MiB cap ──────────────────────────────────────────────────

    #[test]
    fn too_large_response_is_rejected_with_explanatory_envelope() {
        // Inflate the registry past 1 MiB by registering many distinct
        // label sets on a Safe family (raw labels survive
        // serialization; Restricted projection would emit the much
        // shorter labels_hash). 4 KiB-per-series × 256 series = 1 MiB
        // floor; we register 2048 to comfortably exceed the cap.
        classify("bus_too_large_metric", LabelSensitivity::Safe);
        let recorder = StatsRecorderBuilder::new("bus-test")
            .default_cardinality(4096)
            .build();
        for i in 0..2048 {
            // Construct labels with enough bytes per series that
            // the serialized response definitely exceeds 1 MiB.
            // A 512-byte label value × 1 label × 2048 series ≈ 1 MiB
            // of label bytes alone, plus JSON framing.
            let big = "x".repeat(512);
            // Label keys need 'static str; cycle a small set so the
            // distinguishing bytes go into the value (variable).
            let key = "k";
            let label = Label::new(key, format!("{big}-{i}"));
            let metric_key =
                Key::from_parts(SharedString::const_str("bus_too_large_metric"), vec![label]);
            let counter: Counter = recorder.register_counter(
                &metric_key,
                &metrics::Metadata::new("bus_too_large_metric", metrics::Level::INFO, None),
            );
            counter.increment(1);
        }
        let cmd = make_cmd(&[]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "TooLarge");
        assert_eq!(v["limit_bytes"], SNAPSHOT_MAX_RESPONSE_BYTES);
        assert!(v["actual_bytes"].as_u64().unwrap() > SNAPSHOT_MAX_RESPONSE_BYTES as u64);
        let reason = v["reason"].as_str().expect("reason string");
        assert!(
            reason.contains("metric") && reason.contains("labels"),
            "reason explains how to narrow: {reason}"
        );
    }

    #[test]
    fn labels_filter_boolean_value_rejected() {
        let recorder = build_with_counter("bus-test", "bus_labels_boolean_metric", &[], 1);
        let cmd = make_cmd(&[("labels", r#"{"enabled":true}"#)]);
        let (rc, body) = handle_snapshot_bus(&cmd, &recorder);
        assert_eq!(rc, RC_ERR);
        let v = parse_body(&body);
        assert_eq!(v["error"], "ParseError");
        let reason = v["reason"].as_str().expect("reason string");
        assert!(
            reason.contains("string"),
            "reason cites string-only constraint: {reason}"
        );
    }
}
