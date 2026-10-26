// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Filtered statistics snapshots, available to every Bus caller.
//! Label sensitivity still governs on-disk logging; Bus snapshots return raw labels.

use crate::stats::labels_hash::labels_hash as compute_labels_hash;
use crate::stats::recorder::StatsRecorder;
use crate::stats::snapshot::snapshot_from_inner;
use crate::stats::types::{MetricFamily, Series, SeriesLabels, Snapshot};

/// Parsed `<svc>.stats.snapshot` request. The wire-layer parser
/// (slice 5b) constructs this; the dispatch never sees Bus message
/// types directly.
#[derive(Debug, Clone, Default)]
pub struct SnapshotRequest {
    /// `metric?` header: optional glob restricting which metric
    /// families appear in the response. `None` = no family filter
    /// (return everything that passes other filters).
    pub metric: Option<MetricPattern>,
    /// `labels?` header: zero or more label predicates. **All**
    /// must match for a series to appear in the response (AND
    /// semantics — there is no OR form at the Bus surface).
    pub labels: Vec<LabelFilter>,
    /// `labels_hash?` header: zero or more 16-hex-char FxHash
    /// digests. A series matches if its labels_hash (computed via
    /// [`crate::stats::labels_hash`]) is in this set. Empty list =
    /// no labels_hash filter.
    pub labels_hash: Vec<String>,
}

/// Glob pattern over metric family names. The wire-layer parser
/// rejects malformed globs; the dispatch sees only the parsed form.
///
/// The supported shapes mirror the documented examples in plan §4.2
/// (`maild_*`, `*_total`, exact). No middle-`*` form — keeps the
/// matcher trivial and the wire grammar bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricPattern {
    /// Family name must equal this string exactly.
    Exact(String),
    /// Family name must start with this prefix (the substring
    /// before the trailing `*` in `maild_*`).
    Prefix(String),
    /// Family name must end with this suffix (the substring after
    /// the leading `*` in `*_total`).
    Suffix(String),
}

/// One predicate inside a parsed `labels?` filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelFilter {
    /// Series must carry the label key (any value).
    HasKey(String),
    /// Series's label value for `key` must equal `value` exactly.
    KeyEquals { key: String, value: String },
}

/// Capture once and apply all filters to the same snapshot.
pub fn snapshot_dispatch(recorder: &StatsRecorder, request: &SnapshotRequest) -> Snapshot {
    let mut snap = snapshot_from_inner(&recorder.inner);
    snap.metrics
        .retain(|family| family_matches_metric_filter(&family.name, request.metric.as_ref()));
    for family in &mut snap.metrics {
        filter_family(family, request);
    }
    snap.metrics.retain(|family| !family.series.is_empty());
    snap
}

fn family_matches_metric_filter(name: &str, pattern: Option<&MetricPattern>) -> bool {
    match pattern {
        None => true,
        Some(MetricPattern::Exact(s)) => name == s.as_str(),
        Some(MetricPattern::Prefix(p)) => name.starts_with(p.as_str()),
        Some(MetricPattern::Suffix(s)) => name.ends_with(s.as_str()),
    }
}

fn filter_family(family: &mut MetricFamily, request: &SnapshotRequest) {
    // Match hashes against the same raw labels used by JSONL logging.
    family
        .series
        .retain(|s| series_matches_labels_filter(s, &request.labels));
    if !request.labels_hash.is_empty() {
        family
            .series
            .retain(|s| series_matches_labels_hash_filter(s, &request.labels_hash));
    }
}

fn series_matches_labels_filter(series: &Series, filters: &[LabelFilter]) -> bool {
    let SeriesLabels::Raw(map) = &series.labels else {
        // snapshot_from_inner produces only Raw; defensive guard
        // for a future caller that hands in a pre-projected snap.
        return filters.is_empty();
    };
    filters.iter().all(|f| match f {
        LabelFilter::HasKey(k) => map.contains_key(k.as_str()),
        LabelFilter::KeyEquals { key, value } => {
            map.get(key.as_str()).map(|s| s.as_str()) == Some(value.as_str())
        }
    })
}

fn series_matches_labels_hash_filter(series: &Series, accept: &[String]) -> bool {
    let SeriesLabels::Raw(map) = &series.labels else {
        return false;
    };
    let digest = compute_labels_hash(map);
    accept.iter().any(|h| h == &digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::classify::classify;
    use crate::stats::recorder::StatsRecorderBuilder;
    use crate::stats::types::{LabelSensitivity, SeriesValue};
    use metrics::{Counter, Key, Label, Recorder, SharedString};

    /// Build a recorder, register a counter under (name, labels),
    /// increment by `n`, and return the recorder. The helper avoids
    /// the `metrics::with_local_recorder` thread-local seam since
    /// the dispatch operates on the recorder directly.
    fn build_with_counter(
        service: &str,
        name: &'static str,
        labels: &[(&'static str, &'static str)],
        n: u64,
    ) -> crate::stats::recorder::StatsRecorder {
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

    fn req_default() -> SnapshotRequest {
        SnapshotRequest::default()
    }

    #[test]
    fn every_label_filter_returns_raw_labels() {
        let recorder = build_with_counter(
            "dispatch-test",
            "dispatch_open_metric",
            &[("user", "alice")],
            1,
        );
        for metric in [
            None,
            Some(MetricPattern::Exact("dispatch_open_metric".into())),
            Some(MetricPattern::Prefix("dispatch_open_".into())),
            Some(MetricPattern::Suffix("_metric".into())),
        ] {
            let request = SnapshotRequest {
                metric,
                labels: vec![LabelFilter::KeyEquals {
                    key: "user".into(),
                    value: "alice".into(),
                }],
                ..Default::default()
            };
            let snapshot = snapshot_dispatch(&recorder, &request);
            assert_eq!(snapshot.metrics.len(), 1);
            assert!(
                matches!(&snapshot.metrics[0].series[0].labels, SeriesLabels::Raw(labels) if labels.get("user").map(String::as_str) == Some("alice"))
            );
        }
    }

    #[test]
    fn metric_pattern_prefix_and_suffix_match() {
        classify("dispatch_prefix_match_total", LabelSensitivity::Safe);
        let inner = build_with_counter("dispatch-test", "dispatch_prefix_match_total", &[], 1);
        let req_prefix = SnapshotRequest {
            metric: Some(MetricPattern::Prefix("dispatch_prefix_".to_string())),
            ..Default::default()
        };
        let snap = snapshot_dispatch(&inner, &req_prefix);
        assert!(
            snap.metrics
                .iter()
                .any(|m| m.name == "dispatch_prefix_match_total")
        );

        let req_suffix = SnapshotRequest {
            metric: Some(MetricPattern::Suffix("_match_total".to_string())),
            ..Default::default()
        };
        let snap = snapshot_dispatch(&inner, &req_suffix);
        assert!(
            snap.metrics
                .iter()
                .any(|m| m.name == "dispatch_prefix_match_total")
        );

        let req_exclude = SnapshotRequest {
            metric: Some(MetricPattern::Prefix("does_not_exist_".to_string())),
            ..Default::default()
        };
        let snap = snapshot_dispatch(&inner, &req_exclude);
        assert!(snap.metrics.is_empty());
    }

    #[test]
    fn labels_hash_filter_matches_canonical_digest() {
        // The dispatch's labels_hash matching must agree byte-for-byte
        // with the canonical labels_hash helper — operators correlate
        // Bus responses to JSONL records by exact hash. A
        // Safe-classified family is convenient here because we want to
        // assert the digest computed against raw labels (the labels
        // the recorder holds), regardless of projection.
        classify("dispatch_labels_hash_metric", LabelSensitivity::Safe);
        let inner = build_with_counter(
            "dispatch-test",
            "dispatch_labels_hash_metric",
            &[("verdict", "spam")],
            1,
        );
        // Compute the expected digest the way the helper does.
        let mut map = std::collections::BTreeMap::new();
        map.insert("verdict".to_string(), "spam".to_string());
        let expected = crate::stats::labels_hash::labels_hash(&map);
        let req = SnapshotRequest {
            labels_hash: vec![expected.clone()],
            ..Default::default()
        };
        let snap = snapshot_dispatch(&inner, &req);
        let family = snap
            .metrics
            .iter()
            .find(|m| m.name == "dispatch_labels_hash_metric")
            .expect("family present");
        assert_eq!(family.series.len(), 1);

        let req_no_match = SnapshotRequest {
            labels_hash: vec!["0000000000000000".to_string()],
            ..Default::default()
        };
        let snap = snapshot_dispatch(&inner, &req_no_match);
        // Family drops entirely when no series survive filtering.
        assert!(
            !snap
                .metrics
                .iter()
                .any(|m| m.name == "dispatch_labels_hash_metric")
        );
    }

    #[test]
    fn empty_filter_returns_full_snapshot() {
        // Sanity check: a default request returns every family with
        // raw labels. Anti-regression for a future
        // refactor that might accidentally pre-filter on Default.
        let inner = build_with_counter(
            "dispatch-test",
            "dispatch_empty_filter_full_metric",
            &[("k", "v")],
            42,
        );
        let snap = snapshot_dispatch(&inner, &req_default());
        let family = snap
            .metrics
            .iter()
            .find(|m| m.name == "dispatch_empty_filter_full_metric")
            .expect("family present");
        match &family.series[0].value {
            SeriesValue::Counter(v) => assert_eq!(*v, 42),
            other => panic!("expected counter, got {other:?}"),
        }
    }
}
