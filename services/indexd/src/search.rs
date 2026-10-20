// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

// --- In-memory exact search (off the global mutex) ---

/// One-release rollback switch: `MIXOS_INDEXD_SEARCH=sqlite` in the
/// unit environment restores the old sqlite-vec KNN path AND bypasses
/// the in-memory index load/validation at startup — the switch must
/// work even when the loader itself is what's broken. Remove after the
/// in-memory path has soaked.
pub(crate) fn sqlite_search_fallback() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("MIXOS_INDEXD_SEARCH").as_deref() == Ok("sqlite"))
}

/// Hard cap on `SearchRequest.limit` — an unbounded limit performs that
/// many point fetches inside the read gate. Generous versus real
/// callers (context_search asks for ≤10).
pub(crate) const MAX_SEARCH_LIMIT: usize = 200;

/// Bound concurrent scans so they cannot starve inference or memory bandwidth.
/// Two permits: one active scan plus one on deck.
pub(crate) static SEARCH_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Default)]
pub(crate) struct SearchPhases {
    pub(crate) candidate_db_us: u64,
    pub(crate) vector_us: u64,
    pub(crate) fetch_db_us: u64,
}

/// Exact search on the in-memory index, entirely OFF the global
/// AppState mutex: eligibility filters run first as SQL on a dedicated
/// read-only connection (fixing the vec0 filter-after-KNN under-fill
/// defect — results are now exactly `min(limit, eligible)`), then an
/// exact L2 top-k over the candidate slots, then row materialisation
/// inside the same read transaction. The index read guard is held
/// across the whole transaction so a concurrent writer (write lock →
/// commit → patch) can never present mixed db/cache state. Runs inside
/// `spawn_blocking`; never call on the async runtime directly.
pub(crate) fn search_exact(
    db_path: &str,
    index: &std::sync::RwLock<vindex::VectorIndex>,
    query: &[f32],
    limit: usize,
    source_filter: &str,
    metadata_filters: &[MetadataFilter],
    max_response_bytes: usize,
) -> Result<(Vec<SearchResult>, SearchPhases)> {
    let _reader = LongReaderGuard::new();
    let mut phases = SearchPhases::default();
    let vguard = index.read().expect("search index RwLock poisoned");
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    let tx = conn.unchecked_transaction()?;

    // Candidate preselection — the SAME predicate semantics the old
    // path applied post-KNN (json_extract comparisons, LIKE contains),
    // now applied BEFORE distance selection. Sequential `?` binding.
    let t0 = Instant::now();
    let mut sql = String::from("SELECT id FROM chunks WHERE superseded_by IS NULL");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !source_filter.is_empty() {
        sql.push_str(" AND source = ?");
        params.push(Box::new(source_filter.to_string()));
    }
    for filter in metadata_filters {
        let json_path = format!("$.{}", filter.field);
        let op_str = match filter.op {
            FilterOp::Eq => "=",
            FilterOp::Gt => ">",
            FilterOp::Lt => "<",
            FilterOp::Gte => ">=",
            FilterOp::Lte => "<=",
            FilterOp::Contains => "LIKE",
        };
        sql.push_str(&format!(" AND json_extract(metadata, ?) {op_str} ?"));
        params.push(Box::new(json_path));
        if matches!(filter.op, FilterOp::Contains) {
            let pattern = format!("%{}%", filter.value.as_str().unwrap_or(""));
            params.push(Box::new(pattern));
        } else {
            match &filter.value {
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        params.push(Box::new(f));
                    } else {
                        params.push(Box::new(n.as_i64().unwrap_or(0)));
                    }
                }
                serde_json::Value::String(s) => params.push(Box::new(s.clone())),
                other => params.push(Box::new(other.to_string())),
            }
        }
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = tx.prepare(&sql)?;
    let candidates: Vec<i64> = stmt
        .query_map(&*param_refs, |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    phases.candidate_db_us = t0.elapsed().as_micros() as u64;

    // Exact distance top-k over the candidates. A missing candidate is
    // mirror/database divergence — hard error, never a silent skip.
    let t0 = Instant::now();
    let top = vguard
        .top_k(query, &candidates, limit)
        .map_err(|e| anyhow::anyhow!(e))?;
    phases.vector_us = t0.elapsed().as_micros() as u64;

    // Materialise the selected rows inside the same transaction.
    let t0 = Instant::now();
    let mut stmt = tx.prepare(
        "SELECT content, source, metadata, feedback_score, retrieval_count,
                last_retrieved, created
         FROM chunks WHERE id = ?1",
    )?;
    let mut results: Vec<SearchResult> = Vec::with_capacity(top.len());
    let mut response_bytes = 0usize;
    for (id, dist) in top {
        let row = stmt.query_row([id], |row| {
            Ok(SearchResult {
                id,
                distance: dist as f64,
                content: row.get(0)?,
                source: row.get(1)?,
                metadata: row.get(2)?,
                feedback_score: row.get::<_, i64>(3).unwrap_or(0),
                retrieval_count: row.get::<_, i64>(4).unwrap_or(0),
                last_retrieved: row.get::<_, Option<String>>(5).ok().flatten(),
                created: row.get::<_, String>(6).unwrap_or_default(),
            })
        });
        match row {
            Ok(r) => {
                charge_response_item(
                    &mut response_bytes,
                    &[&r.content, &r.source, &r.metadata, &r.created],
                    max_response_bytes,
                )?;
                results.push(r);
            }
            // Candidate vanished between selection and fetch is
            // impossible while we hold the index read guard (writers
            // block on the write lock) — treat as a hard error.
            Err(e) => anyhow::bail!("search fetch failed for chunk {id}: {e}"),
        }
    }
    drop(stmt);
    tx.finish()?;
    phases.fetch_db_us = t0.elapsed().as_micros() as u64;
    drop(vguard);

    // Re-sort by adjusted distance — identical semantics to the old
    // path: feedback boost - implicit negative - staleness penalty
    // applied over the top-`limit` set.
    let now_days = days_since_epoch_utc();
    results.sort_by(|a, b| {
        let adj_a = adjusted_distance(a, now_days);
        let adj_b = adjusted_distance(b, now_days);
        adj_a
            .partial_cmp(&adj_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok((results, phases))
}

pub(crate) fn days_since_epoch_utc() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86400) as i64)
        .unwrap_or(0)
}

/// Parse "YYYY-MM-DD HH:MM:SS" (SQLite datetime('now')) or "YYYY-MM-DDTHH:MM:SS" to days-since-epoch.
/// Returns None on any parse failure (no date crate — naive conversion using days-per-month).
pub(crate) fn sqlite_datetime_to_days(s: &str) -> Option<i64> {
    if s.len() < 10 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    // Days from epoch: count years+leap days, month days, day of month.
    // Using the common algorithm (Howard Hinnant's date algorithms, shifted-year form).
    let y = y - (if m <= 2 { 1 } else { 0 });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// Compute ranking-adjusted distance for a search result.
/// Lower is better. Combines: feedback_score boost, implicit negative signal, staleness penalty.
pub(crate) fn adjusted_distance(r: &SearchResult, now_days: i64) -> f64 {
    let feedback_boost = r.feedback_score as f64 * 0.05;

    // Implicit negative: retrieved >3 times with non-positive feedback = weak penalty.
    let implicit_penalty = if r.retrieval_count > 3 && r.feedback_score <= 0 {
        (r.retrieval_count as f64 - 3.0) * 0.01
    } else {
        0.0
    };

    // Staleness penalty: never-retrieved old chunks, or long-unretrieved chunks.
    let staleness_penalty = match r.last_retrieved.as_deref() {
        None => {
            // Never retrieved — penalize if chunk is old.
            if let Some(created_days) = sqlite_datetime_to_days(&r.created) {
                let age = now_days - created_days;
                if age > 90 { 0.03 } else { 0.0 }
            } else {
                0.0
            }
        }
        Some(lr) => {
            if let Some(lr_days) = sqlite_datetime_to_days(lr) {
                let since = now_days - lr_days;
                if since > 180 { 0.02 } else { 0.0 }
            } else {
                0.0
            }
        }
    };

    r.distance - feedback_boost + implicit_penalty + staleness_penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_implicit_negative_and_staleness_keep_cosmix_weights() {
        let mut result = SearchResult {
            id: 1, content: String::new(), source: "doc".into(), metadata: "{}".into(),
            distance: 1.0, feedback_score: 2, retrieval_count: 10,
            last_retrieved: None, created: "2020-01-01 00:00:00".into(),
        };
        let now = sqlite_datetime_to_days("2026-10-20 00:00:00").unwrap();
        assert!((adjusted_distance(&result, now) - 0.93).abs() < 1e-12);
        result.feedback_score = 0;
        assert!((adjusted_distance(&result, now) - 1.10).abs() < 1e-12);
        result.retrieval_count = 3;
        result.last_retrieved = Some("2020-01-01 00:00:00".into());
        assert!((adjusted_distance(&result, now) - 1.02).abs() < 1e-12);
        result.last_retrieved = Some("2026-10-20T00:00:00".into());
        assert_eq!(adjusted_distance(&result, now), 1.0);
    }
}

