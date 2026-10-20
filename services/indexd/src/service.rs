// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

/// Wire name of a parsed request, for metrics/log attribution.
pub(crate) fn action_name(req: &Request) -> &'static str {
    match req {
        Request::Embed(_) => "embed",
        Request::Store(_) => "store",
        Request::Search(_) => "search",
        Request::Update(_) => "update",
        Request::Delete(_) => "delete",
        Request::List(_) => "list",
        Request::Feedback(_) => "feedback",
        Request::Supersede(_) => "supersede",
        Request::Stale(_) => "stale",
        Request::IndexFile(_) => "index_file",
        Request::Stats => "stats",
    }
}

pub(crate) async fn process_request(
    input: &str,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    ctx: &RequestContext,
) -> String {
    let config = state.lock().await.config.clone();
    let seq = metrics::REQUEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let bytes = input.len() as u64;
    let t0 = Instant::now();
    let mut t = ReqTiming::default();

    if input.len() > config.service.max_request_line_bytes {
        let response =
            json_code_error("request_too_large", "request exceeds configured byte limit");
        record_request(
            seq,
            "invalid",
            ctx,
            bytes,
            &t,
            t0.elapsed(),
            Outcome::Invalid,
            &response,
        );
        return response;
    }

    let req: Request = match serde_json::from_str(input) {
        Ok(r) => r,
        Err(e) => {
            let response = json_error(&format!("invalid request: {e}"));
            // Charge a malformed-but-recognisable action to its own
            // bucket (outcome=invalid) so its real traffic volume isn't
            // understated; only actionless garbage lands in `invalid`.
            record_request(
                seq,
                &metrics::action_of_invalid(input),
                ctx,
                bytes,
                &t,
                t0.elapsed(),
                Outcome::Invalid,
                &response,
            );
            return response;
        }
    };
    let action = action_name(&req);

    let response = match req {
        Request::Embed(req) => handle_embed(req, state, activity_tx, &mut t).await,
        Request::Store(req) => handle_store(req, state, activity_tx, &mut t).await,
        Request::Search(req) => handle_search(req, state, activity_tx, &mut t).await,
        Request::Update(req) => handle_update(req, state, activity_tx, &mut t).await,
        Request::Delete(req) => handle_delete(req, state, &mut t).await,
        Request::List(req) => handle_list(req, state, &mut t).await,
        Request::Feedback(req) => handle_feedback(req, state, &mut t).await,
        Request::Supersede(req) => handle_supersede(req, state, &mut t).await,
        Request::Stale(req) => handle_stale(req, state, &mut t).await,
        Request::IndexFile(req) => {
            let max_file_bytes = config.service.max_file_bytes;
            if req
                .content
                .as_ref()
                .is_some_and(|content| content.len() > max_file_bytes)
            {
                json_error(&format!(
                    "caller-supplied content exceeds max {max_file_bytes} bytes"
                ))
            } else if req.background {
                // Opt-in fire-and-forget: enqueue the job and ack immediately
                // so a slow multi-section embed can't blow the caller's Bus
                // `send` timeout into a false rc=10. The worker drains the
                // queue and runs the *same* `handle_index_file` path. The
                // envelope carries origin context so the worker's CPU is
                // attributed to the real requester, not left orphaned.
                let filepath = expand_home(&req.path);
                let file = std::path::Path::new(&filepath)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or(filepath);
                // Reserve owned bytes before transferring the request into the
                // bounded channel. Both count and byte overflow are explicit
                // queue_full responses; no work is silently discarded.
                let (job_tx, queue_budget) = {
                    let guard = lock_timed(state, &mut t).await;
                    (guard.job_tx.clone(), guard.queue_budget.clone())
                };
                let owned_bytes = index_job_owned_bytes(&req);
                let reservation = match queue_budget.try_reserve(owned_bytes) {
                    Ok(reservation) => reservation,
                    Err(()) => {
                        return finish_request(
                            seq,
                            action,
                            ctx,
                            bytes,
                            &t,
                            t0,
                            json_code_error(
                                "queue_full",
                                "background index queue count/byte budget exhausted",
                            ),
                        );
                    }
                };
                let job = IndexJob {
                    req,
                    seq,
                    peer: format!("{}:{}", ctx.transport, ctx.peer),
                    bytes,
                    enqueued: Instant::now(),
                    attempt: 1,
                    reservation,
                };
                match job_tx.try_send(job) {
                    Ok(()) => serde_json::json!({
                        "accepted": true,
                        "queued": true,
                        "file": file,
                    })
                    .to_string(),
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_job)) => json_code_error(
                        "queue_full",
                        "background index queue count budget exhausted",
                    ),
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(job)) => {
                        warn!("background job queue closed; indexing {file} synchronously");
                        handle_index_file(job.req, state, activity_tx, &mut t).await
                    }
                }
            } else {
                handle_index_file(req, state, activity_tx, &mut t).await
            }
        }
        Request::Stats => handle_stats(state, &mut t).await,
    };
    let response = if response.len() > config.service.max_response_bytes {
        json_code_error(
            "response_too_large",
            "serialised response exceeds configured byte limit",
        )
    } else {
        response
    };

    record_request(
        seq,
        action,
        ctx,
        bytes,
        &t,
        t0.elapsed(),
        classify_response(&response),
        &response,
    );
    response
}
pub(crate) async fn handle_embed(
    req: EmbedRequest,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> String {
    let config = state.lock().await.config.clone();
    if req.texts.len() > MAX_TEXTS_PER_REQUEST {
        return json_error(&format!(
            "too many texts: {} (max {MAX_TEXTS_PER_REQUEST})",
            req.texts.len()
        ));
    }
    if let Err(e) = validate_embedding_budget(&req.texts, &req.prefix, &config.service) {
        return json_error(&e);
    }
    t.work = req.texts.len() as u64;
    match compute_embeddings(state, &req.texts, &req.prefix, activity_tx, t).await {
        Ok(embeddings) => serde_json::to_string(&EmbedResponse { embeddings }).unwrap(),
        Err(e) => json_error(&e),
    }
}

/// Validate source_types policy from the loaded strict-data configuration.
pub(crate) fn validate_store_entry(
    source: &str,
    metadata_json: &str,
    cfg: &Config,
) -> Result<(), String> {
    // Skip validation for empty metadata (legacy/test path) or unset source.
    if metadata_json.is_empty() || source.is_empty() {
        return Ok(());
    }
    let meta: serde_json::Value = serde_json::from_str(metadata_json)
        .map_err(|e| format!("metadata is not valid JSON: {e}"))?;
    let has = |f: &str| meta.get(f).is_some_and(|v| !v.is_null());

    let spec = cfg.source_types.get(source).ok_or_else(|| {
        format!("unknown source type: {source} (add to the loaded indexd config)")
    })?;
    for f in &spec.required {
        if !has(f) {
            return Err(format!("{source} metadata missing required field: {f}"));
        }
    }

    if let Some(date_field) = &spec.date_field {
        let date_str = meta.get(date_field).and_then(|v| v.as_str()).unwrap_or("");
        if !is_valid_ymd(date_str) {
            return Err(format!(
                "{source} {date_field} '{date_str}' not valid YYYY-MM-DD"
            ));
        }
    }

    Ok(())
}

pub(crate) async fn handle_store(
    req: StoreRequest,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> String {
    let config = state.lock().await.config.clone();
    if req.texts.len() > MAX_TEXTS_PER_REQUEST {
        return json_error(&format!(
            "too many texts: {} (max {MAX_TEXTS_PER_REQUEST})",
            req.texts.len()
        ));
    }
    let prefix = "search_document: ";
    if let Err(e) = validate_embedding_budget(&req.texts, prefix, &config.service) {
        return json_error(&e);
    }
    // Layer separation enforcement: validate each entry's metadata
    for meta_str in req.metadata.iter() {
        if let Err(e) = validate_store_entry(&req.source, meta_str, &config) {
            return json_error(&format!("validation failed: {e}"));
        }
    }

    // Optimistic read-only duplicate probe: normal unchanged re-indexing does
    // no inference. The final IMMEDIATE transaction revalidates every skipped
    // item before any metadata/insert write. A vanished duplicate rolls the
    // whole attempt back; only those vanished items are embedded off-lock.
    let preflight_ids = {
        let guard = lock_timed(state, t).await;
        let db_t0 = Instant::now();
        let result = guard.db.preflight_duplicate_ids(&req.texts, &req.source);
        t.db_us += db_t0.elapsed().as_micros() as u64;
        match result {
            Ok(ids) => ids,
            Err(e) => return json_error(&format!("store preflight failed: {e}")),
        }
    };
    let mut embeddings: Vec<Option<Vec<f32>>> = vec![None; req.texts.len()];
    let apparent_misses: Vec<usize> = preflight_ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| id.is_none().then_some(i))
        .collect();
    if !apparent_misses.is_empty() {
        let texts: Vec<String> = apparent_misses
            .iter()
            .map(|&i| req.texts[i].clone())
            .collect();
        let computed = match compute_embeddings(state, &texts, prefix, activity_tx, t).await {
            Ok(embeddings) => embeddings,
            Err(e) => return json_error(&e),
        };
        for (embedding, &i) in computed.into_iter().zip(&apparent_misses) {
            embeddings[i] = Some(embedding);
        }
    }

    const MAX_VANISH_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_VANISH_ATTEMPTS {
        let guard = lock_timed(state, t).await;
        let db_t0 = Instant::now();
        let stored_res = guard.db.store_revalidated(
            &embeddings,
            &preflight_ids,
            &req.texts,
            &req.source,
            &req.metadata,
        );
        t.db_us += db_t0.elapsed().as_micros() as u64;
        match stored_res {
            Ok(StoreAttempt::Committed { ids, duplicates }) => {
                let stored = ids.len() - duplicates;
                t.work = ids.len() as u64;
                return serde_json::to_string(&StoreResponse {
                    stored,
                    duplicates,
                    ids,
                })
                .unwrap();
            }
            Ok(StoreAttempt::NeedsEmbeddings(vanished)) => {
                drop(guard);
                if attempt == MAX_VANISH_ATTEMPTS {
                    return json_code_error(
                        "duplicate_vanished",
                        "store abandoned after repeated duplicate-delete races; transaction rolled back",
                    );
                }
                let missing: Vec<usize> = vanished
                    .into_iter()
                    .filter(|&i| embeddings[i].is_none())
                    .collect();
                if !missing.is_empty() {
                    let texts: Vec<String> =
                        missing.iter().map(|&i| req.texts[i].clone()).collect();
                    let computed =
                        match compute_embeddings(state, &texts, prefix, activity_tx, t).await {
                            Ok(embeddings) => embeddings,
                            Err(e) => return json_error(&e),
                        };
                    for (embedding, &i) in computed.into_iter().zip(&missing) {
                        embeddings[i] = Some(embedding);
                    }
                }
            }
            Err(e) => return json_error(&format!("store failed: {e}")),
        }
    }
    unreachable!("bounded store loop always returns or continues")
}

pub(crate) async fn handle_search(
    req: SearchRequest,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> String {
    let config = state.lock().await.config.clone();
    if req.metadata_filter.len() > MAX_METADATA_FILTERS {
        return json_error(&format!(
            "too many metadata filters: {} (max {MAX_METADATA_FILTERS})",
            req.metadata_filter.len()
        ));
    }
    let limit = req.limit.min(MAX_SEARCH_LIMIT);
    let prefix = "search_query: ";

    // Off-lock embed (compute_embeddings handles the cache + model load).
    let query_emb = match compute_embeddings(
        state,
        std::slice::from_ref(&req.query),
        prefix,
        activity_tx,
        t,
    )
    .await
    {
        Ok(mut embs) if !embs.is_empty() => embs.remove(0),
        Ok(_) => return json_error("empty query"),
        Err(e) => return json_error(&e),
    };

    if sqlite_search_fallback() {
        // Rollback path: old sqlite-vec KNN under the global mutex.
        let guard = lock_timed(state, t).await;
        let db_t0 = Instant::now();
        let search_res = guard.db.search(
            &query_emb,
            limit,
            &req.source,
            &req.metadata_filter,
            config.service.max_response_bytes,
        );
        return match search_res {
            Ok(results) => {
                // Fire-and-forget: track that these chunks were retrieved
                // (implicit feedback signal).
                let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
                guard.db.mark_retrieved(&ids);
                t.db_us += db_t0.elapsed().as_micros() as u64;
                t.work = results.len() as u64;
                serde_json::to_string(&SearchResponse { results }).unwrap()
            }
            Err(e) => {
                t.db_us += db_t0.elapsed().as_micros() as u64;
                json_error(&format!("search failed: {e}"))
            }
        };
    }

    // In-memory exact path: the global mutex is held only long enough
    // to clone the handle, then the scan runs in spawn_blocking against
    // the index + a read-only connection. Admission-gated: the gate
    // wait is charged to lock_wait_us.
    let gate_t0 = Instant::now();
    let _permit = SEARCH_GATE
        .acquire()
        .await
        .expect("search gate semaphore closed");
    t.lock_wait_us += gate_t0.elapsed().as_micros() as u64;
    let (db_path, index) = {
        let guard = lock_timed(state, t).await;
        (guard.db_path.clone(), guard.db.search_index.clone())
    };
    let source = req.source;
    let metadata_filter = req.metadata_filter;
    let max_response_bytes = config.service.max_response_bytes;
    let joined = tokio::task::spawn_blocking(move || {
        search_exact(
            &db_path,
            &index,
            &query_emb,
            limit,
            &source,
            &metadata_filter,
            max_response_bytes,
        )
    })
    .await;
    match joined {
        Ok(Ok((results, phases))) => {
            t.db_us += phases.candidate_db_us + phases.fetch_db_us;
            t.vector_us += phases.vector_us;
            // Retrieval bookkeeping is the only remaining global-mutex
            // work — a small indexed UPDATE.
            let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
            {
                let guard = lock_timed(state, t).await;
                let db_t0 = Instant::now();
                guard.db.mark_retrieved(&ids);
                t.db_us += db_t0.elapsed().as_micros() as u64;
            }
            t.work = results.len() as u64;
            serde_json::to_string(&SearchResponse { results }).unwrap()
        }
        Ok(Err(e)) => json_error(&format!("search failed: {e}")),
        Err(join) => json_error(&format!("search worker failed: {join}")),
    }
}

pub(crate) async fn handle_update(
    req: UpdateRequest,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> String {
    let prefix = "search_document: ";

    // Re-embed off-lock if content changed (compute_embeddings caches).
    // An empty result for a requested content embed is an error, not a silent
    // skip — persisting new content with a stale vector would corrupt search.
    let new_embedding = if let Some(ref content) = req.content {
        match compute_embeddings(state, std::slice::from_ref(content), prefix, activity_tx, t).await
        {
            Ok(mut embs) if !embs.is_empty() => Some(embs.remove(0)),
            Ok(_) => return json_error("embedding failed: empty result for content"),
            Err(e) => return json_error(&e),
        }
    } else {
        None
    };

    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let update_res = guard.db.update(
        req.id,
        req.content.as_deref(),
        req.metadata.as_deref(),
        req.source.as_deref(),
        new_embedding.as_deref(),
    );
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match update_res {
        Ok(updated) => {
            t.work = updated as u64;
            serde_json::to_string(&UpdateResponse {
                updated,
                re_embedded: new_embedding.is_some(),
            })
            .unwrap()
        }
        Err(e) => json_error(&format!("update failed: {e}")),
    }
}

pub(crate) async fn handle_delete(
    req: DeleteRequest,
    state: &Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> String {
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.delete(&req.ids);
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok(deleted) => {
            t.work = deleted as u64;
            serde_json::to_string(&DeleteResponse { deleted }).unwrap()
        }
        Err(e) => json_error(&format!("delete failed: {e}")),
    }
}

pub(crate) async fn handle_list(
    req: ListRequest,
    state: &Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> String {
    let config = state.lock().await.config.clone();
    let limit = match sqlite_count(clamp_list_limit(req.limit), "list.limit") {
        Ok(value) => value,
        Err(e) => return json_error(&e),
    };
    let offset = match sqlite_count(req.offset, "list.offset") {
        Ok(value) => value,
        Err(e) => return json_error(&e),
    };
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.list(
        &req.source,
        limit,
        offset,
        config.service.max_response_bytes,
    );
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok((items, total)) => {
            t.work = items.len() as u64;
            serde_json::to_string(&ListResponse { items, total }).unwrap()
        }
        Err(e) => json_error(&format!("list failed: {e}")),
    }
}

pub(crate) async fn handle_feedback(
    req: FeedbackRequest,
    state: &Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> String {
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.feedback(req.id, req.useful);
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok(new_score) => {
            t.work = 1;
            serde_json::json!({"ok": true, "id": req.id, "feedback_score": new_score}).to_string()
        }
        Err(e) => json_error(&format!("feedback failed: {e}")),
    }
}

pub(crate) async fn handle_supersede(
    req: SupersedeRequest,
    state: &Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> String {
    if !req.reason.is_empty() {
        info!(
            old_id = req.old_id,
            new_id = req.new_id,
            reason = %req.reason,
            "chunk supersede"
        );
    } else {
        info!(old_id = req.old_id, new_id = req.new_id, "chunk supersede");
    }
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.supersede(req.old_id, req.new_id);
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok(updated) => {
            t.work = updated as u64;
            serde_json::json!({
                "ok": true,
                "old_id": req.old_id,
                "new_id": req.new_id,
                "updated": updated,
            })
            .to_string()
        }
        Err(e) => json_error(&format!("supersede failed: {e}")),
    }
}

pub(crate) async fn handle_stale(
    req: StaleRequest,
    state: &Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> String {
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.stale_query(&req);
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok(resp) => {
            t.work = (resp.never_retrieved_old.len()
                + resp.low_value.len()
                + resp.long_dormant.len()) as u64;
            serde_json::to_string(&resp).unwrap()
        }
        Err(e) => json_error(&format!("stale query failed: {e}")),
    }
}
pub(crate) async fn handle_stats(state: &Arc<Mutex<AppState>>, t: &mut ReqTiming) -> String {
    let guard = lock_timed(state, t).await;
    let db_t0 = Instant::now();
    let res = guard.db.stats(&guard.db_path);
    t.db_us += db_t0.elapsed().as_micros() as u64;
    match res {
        Ok(mut stats) => {
            stats.model_loaded = guard.model.is_some();
            stats.model_circuit = guard.model_breaker.state_name().to_string();
            stats.embed_circuit = guard.embed_breaker.state_name().to_string();
            stats.embed_cache_entries = guard.embed_cache.entries.len();
            stats.embed_cache_hits = guard.embed_cache.hits;
            stats.embed_cache_misses = guard.embed_cache.misses;
            let runtime = metrics::runtime_snapshot();
            stats.queue_depth = runtime.queue_depth;
            stats.queued_bytes = runtime.queued_bytes;
            stats.inference_in_flight = runtime.inference_in_flight;
            stats.timed_out_still_running = runtime.timed_out_still_running;
            stats.model_generation = runtime.model_generation;
            stats.rss_before_load_bytes = runtime.rss_before_load_bytes;
            stats.rss_after_load_bytes = runtime.rss_after_load_bytes;
            stats.rss_after_unload_bytes = runtime.rss_after_unload_bytes;
            // Snapshot + serialisation happen OFF the global lock — the
            // counters have their own synchronisation.
            drop(guard);
            // Attach the per-action counters so external samplers can
            // attribute CPU to workload without scraping the journal.
            let mut v = serde_json::to_value(&stats).unwrap();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("request_metrics".into(), METRICS.snapshot());
            }
            v.to_string()
        }
        Err(e) => json_error(&format!("stats failed: {e}")),
    }
}

pub(crate) fn json_error(msg: &str) -> String {
    serde_json::to_string(&ErrorResponse {
        error: msg.to_string(),
        code: None,
    })
    .unwrap()
}

pub(crate) fn json_code_error(code: &str, msg: &str) -> String {
    serde_json::to_string(&ErrorResponse {
        error: msg.to_string(),
        code: Some(code.to_string()),
    })
    .unwrap()
}

const SLOW_REQUEST_MS: u128 = 250;
// Keep the original explicit timing and request-context arguments together.
#[allow(clippy::too_many_arguments)]
fn record_request(
    seq: u64,
    action: &str,
    ctx: &RequestContext,
    bytes: u64,
    t: &ReqTiming,
    elapsed: Duration,
    outcome: Outcome,
    response: &str,
) {
    let elapsed_us = elapsed.as_micros() as u64;
    METRICS
        .for_action(action)
        .record(t, elapsed_us, bytes, outcome);

    if matches!(outcome, Outcome::Error | Outcome::Invalid) {
        // Error bodies are indexd-generated (json_error), not user data;
        // truncate defensively anyway.
        let brief: String = response.chars().take(200).collect();
        warn!(
            seq,
            action,
            transport = ctx.transport,
            peer = %ctx.peer,
            bytes,
            outcome = outcome.as_str(),
            elapsed_ms = elapsed.as_millis() as u64,
            error = %brief,
            "indexd_request_failed"
        );
        return;
    }
    if matches!(action, "stats") {
        // Sampler-safe: successful metrics reads are counted, never
        // logged (a minutely poller would add 1,440 noise lines/day).
        return;
    }
    // Only pure reads may demote to debug — a cache-hit store/update
    // still mutated durable state and stays at info per the policy.
    let cheap_read =
        matches!(action, "list") || (t.cache_hit && matches!(action, "embed" | "search"));
    let debug_level = cheap_read && !t.cold_model && elapsed.as_millis() < SLOW_REQUEST_MS;
    macro_rules! emit {
        ($lvl:ident) => {
            tracing::$lvl!(
                seq,
                action,
                transport = ctx.transport,
                peer = %ctx.peer,
                bytes,
                outcome = outcome.as_str(),
                cold_model = t.cold_model,
                cache_hit = t.cache_hit,
                work = t.work,
                lock_wait_us = t.lock_wait_us,
                model_load_us = t.model_load_us,
                embed_us = t.embed_us,
                db_us = t.db_us,
                vector_us = t.vector_us,
                elapsed_ms = elapsed.as_millis() as u64,
                "indexd_request_complete"
            )
        };
    }
    if debug_level {
        emit!(debug);
    } else {
        emit!(info);
    }
}

fn finish_request(
    seq: u64,
    action: &str,
    ctx: &RequestContext,
    bytes: u64,
    t: &ReqTiming,
    started: Instant,
    response: String,
) -> String {
    record_request(
        seq,
        action,
        ctx,
        bytes,
        t,
        started.elapsed(),
        classify_response(&response),
        &response,
    );
    response
}

/// Lock the global state, charging the wait to the request's timing.
/// Mutex wait is tracked separately from `db_us` so lock contention
/// can't hide inside apparent SQLite time.
pub(crate) async fn lock_timed<'a>(
    state: &'a Arc<Mutex<AppState>>,
    t: &mut ReqTiming,
) -> tokio::sync::MutexGuard<'a, AppState> {
    let t0 = Instant::now();
    let guard = state.lock().await;
    t.lock_wait_us += t0.elapsed().as_micros() as u64;
    guard
}
// This state is private to the engine; requests never hold it across inference.
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) model: Option<Arc<EmbedModel>>,
    pub(crate) chunk_tokenizer: Option<Arc<ChunkTokenizer>>,
    pub(crate) dtype: DType,
    pub(crate) db: VectorDb,
    pub(crate) db_path: String,
    pub(crate) idle_timeout_secs: u64,
    pub(crate) model_breaker: CircuitBreaker,
    pub(crate) embed_breaker: CircuitBreaker,
    pub(crate) embed_cache: EmbeddingCache,
    pub(crate) inference_gate: Arc<tokio::sync::Semaphore>,
    pub(crate) job_tx: tokio::sync::mpsc::Sender<IndexJob>,
    pub(crate) queue_budget: Arc<QueueBudget>,
}

/// Transport-independent service. Construct inside a Tokio runtime.
/// Each verb accepts its original JSON body and returns its original JSON reply.
pub struct Engine {
    state: Arc<Mutex<AppState>>,
    activity_tx: tokio::sync::mpsc::Sender<()>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

macro_rules! verbs {
    ($($name:ident),* $(,)?) => {$ (
        pub async fn $name(&self, body: serde_json::Value) -> serde_json::Value {
            self.call(stringify!($name), body).await
        }
    )*};
}

impl Engine {
    verbs!(
        embed, store, search, update, delete, list, feedback, supersede, stale, stats, index_file
    );

    async fn call(&self, action: &str, body: serde_json::Value) -> serde_json::Value {
        let mut object = match body {
            serde_json::Value::Object(object) => object,
            _ => return serde_json::json!({"error": "request body must be an object"}),
        };
        object.insert("action".into(), action.into());
        let ctx = RequestContext {
            transport: "engine",
            peer: String::new(),
        };
        let response = process_request(
            &serde_json::Value::Object(object).to_string(),
            &self.state,
            &self.activity_tx,
            &ctx,
        )
        .await;
        serde_json::from_str(&response).expect("handlers produce JSON")
    }

    pub fn open(config: Config) -> Result<Self> {
        config.validate()?;
        let svc = config.service.clone();
        let db_path = svc.vectors_db.clone();
        let db = VectorDb::open(
            &db_path,
            vindex::VectorDtype::parse(&svc.vindex_dtype).map_err(anyhow::Error::msg)?,
            svc.journal_size_limit_bytes,
        )?;
        let (job_tx, mut job_rx) =
            tokio::sync::mpsc::channel::<IndexJob>(svc.background_queue_max_jobs);
        let queue_budget = QueueBudget::new(
            svc.background_queue_max_jobs,
            svc.background_queue_max_bytes,
        );
        let state = Arc::new(Mutex::new(AppState {
            config: Arc::new(config),
            model: None,
            chunk_tokenizer: None,
            dtype: if svc.dtype == "f32" {
                DType::F32
            } else {
                DType::F16
            },
            db,
            db_path,
            idle_timeout_secs: svc.idle_timeout_secs,
            model_breaker: CircuitBreaker::new(2, Duration::from_secs(60)),
            embed_breaker: CircuitBreaker::new(3, Duration::from_secs(30)),
            embed_cache: EmbeddingCache::new(),
            inference_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            job_tx: job_tx.clone(),
            queue_budget: queue_budget.clone(),
        }));
        let (activity_tx, mut activity_rx) = tokio::sync::mpsc::channel::<()>(16);
        let background_retry_max_attempts = svc.background_retry_max_attempts.max(1);
        let background_retry_initial_backoff_secs =
            svc.background_retry_initial_backoff_secs.max(1);
        let mut tasks = Vec::new();

        let checkpoint_state = state.clone();
        let checkpoint_budget = queue_budget.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                CHECKPOINT_RETRY_NOTIFY.notified().await;
                while checkpoint_state.lock().await.db.checkpoint_owed.is_owed() {
                    tokio::time::sleep(Duration::from_secs(300)).await;
                    if LONG_READERS.load(Ordering::Acquire) != 0 || !checkpoint_budget.is_empty() {
                        continue;
                    }
                    let guard = checkpoint_state.lock().await;
                    if let Err(e) = guard.db.checkpoint_truncate() {
                        guard.db.mark_checkpoint_owed();
                        tracing::debug!("WAL checkpoint retry failed: {e}");
                    }
                }
            }
        }));
        // Spawn idle watchdog
        let watchdog_state = state.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if activity_rx.recv().await.is_none() {
                    break;
                }
                while activity_rx.try_recv().is_ok() {}

                let timeout = watchdog_state.lock().await.idle_timeout_secs;
                // 0 = never unload. Don't run the countdown at all; just go back
                // to waiting for the next activity ping (which we drain above).
                if timeout == 0 {
                    continue;
                }
                let mut idle_remaining = timeout;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            idle_remaining = idle_remaining.saturating_sub(1);
                            if idle_remaining == 0 {
                                let mut guard = watchdog_state.lock().await;
                                if guard.inference_gate.available_permits() == 0 {
                                    // A timed-out blocking worker may still own an
                                    // Arc<Model>. Never drop the state Arc while
                                    // that permit is held: a later request could
                                    // otherwise load a second full model.
                                    idle_remaining = 1;
                                    continue;
                                }
                                if guard.model.is_some() {
                                    info!("model idle for {timeout}s, unloading to free memory");
                                    guard.model = None;
                                    drop(guard);
                                    // SAFETY: zero requests release of unused allocator pages.
                                    unsafe { libc::malloc_trim(0); }
                                    metrics::RSS_AFTER_UNLOAD_BYTES
                                        .store(rss_bytes(), Ordering::Relaxed);
                                }
                                break;
                            }
                        }
                        result = activity_rx.recv() => {
                            if result.is_none() {
                                return;
                            }
                            while activity_rx.try_recv().is_ok() {}
                            idle_remaining = timeout;
                        }
                    }
                }
            }
        }));

        // Spawn the single background indexing worker. It owns `job_rx` and
        // drains the queue serially for the daemon's lifetime, reusing the exact
        // synchronous `handle_index_file` path (so background and foreground
        // indexing are byte-for-byte identical). `recv()` returns `None` only
        // once every `job_tx` clone is dropped (process teardown), at which point
        // the loop exits cleanly. Jobs are owned `IndexFileRequest` values, so the
        // future is `Send + 'static`.
        let worker_state = state.clone();
        let worker_activity_tx = activity_tx.clone();
        let worker_queue_budget = queue_budget.clone();
        let worker_job_tx = job_tx.clone();
        tasks.push(tokio::spawn(async move {
        while let Some(mut job) = job_rx.recv().await {
            let path = job.req.path.clone();
            let queue_us = job.enqueued.elapsed().as_micros() as u64;
            let queue_ms = queue_us / 1000;
            // The foreground request already returned its ack; without this
            // start record a stuck job leaves no evidence in the journal.
            info!(
                seq = job.seq,
                path = %path,
                queue_ms,
                peer = %job.peer,
                "indexd_job_started"
            );
            let ctx = RequestContext {
                transport: "background",
                peer: job.peer.clone(),
            };
            let t0 = Instant::now();
            let mut t = ReqTiming {
                queue_us,
                ..Default::default()
            };
            let max_attempts = background_retry_max_attempts;
            let resp =
                handle_index_file(job.req.clone(), &worker_state, &worker_activity_tx, &mut t)
                    .await;
            if transient_background_failure(&resp) {
                if job.attempt < max_attempts {
                    let backoff = background_retry_backoff(
                        background_retry_initial_backoff_secs,
                        job.attempt,
                    );
                    warn!(
                        seq = job.seq,
                        path = %path,
                        attempt = job.attempt,
                        retry_in_secs = backoff.as_secs(),
                        error = %resp,
                        "indexd background job deferred after transient inference failure"
                    );
                    job.attempt += 1;
                    let deferred_tx = worker_job_tx.clone();
                    let deferred_state = worker_state.clone();
                    let deferred_queue_budget = worker_queue_budget.clone();
                    tokio::spawn(async move {
                        let failure =
                            defer_background_job(job, deferred_tx, backoff, max_attempts).await;
                        let (mut abandoned, reason) = match failure {
                            Ok(()) => return,
                            Err(DeferredRequeueFailure::Closed(job)) => {
                                (job, "background job queue closed".to_string())
                            }
                            Err(DeferredRequeueFailure::Full { job, attempts }) => (
                                job,
                                format!(
                                    "background job queue remained full for {attempts} re-submit attempts"
                                ),
                            ),
                        };
                        error!(
                            seq = abandoned.seq,
                            path = %abandoned.req.path,
                            attempt = abandoned.attempt,
                            reason,
                            "indexd background job ABANDONED during deferred re-submit"
                        );
                        release_background_reservation(
                            &mut abandoned,
                            &deferred_state,
                            &deferred_queue_budget,
                        )
                        .await;
                    });
                    continue;
                } else {
                    error!(
                        seq = job.seq,
                        path = %path,
                        attempts = job.attempt,
                        error = %resp,
                        "indexd background job ABANDONED after transient inference failures"
                    );
                }
            }
            let outcome = classify_response(&resp);
            // On failure, record_request below emits the structured warn
            // (indexd_request_failed) carrying the error text, and the
            // start record above already carries the path — no separate
            // duplicate warn.
            record_request(
                job.seq,
                "index_file_job",
                &ctx,
                job.bytes,
                &t,
                t0.elapsed(),
                outcome,
                &resp,
            );
            // The reservation intentionally remains live for every retry, so
            // accepted work cannot evade the queue's byte/count budget while
            // deferred. Release only on success or loud final abandonment.
            release_background_reservation(&mut job, &worker_state, &worker_queue_budget).await;
        }
        info!("background indexing worker exiting (job queue closed)");
    }));

        Ok(Self {
            state,
            activity_tx,
            tasks,
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;
    use serde_json::json;

    fn engine(tag: &str) -> (Engine, PathBuf) {
        let dir = temp_dir(tag);
        let mut config = Config::default();
        config.service.vectors_db = dir.join("vectors.db").to_string_lossy().into_owned();
        config.service.model_dir = dir.join("no-model").to_string_lossy().into_owned();
        (Engine::open(config).unwrap(), dir)
    }

    async fn cache(engine: &Engine, text: &str, prefix: &str, value: f32) {
        engine
            .state
            .lock()
            .await
            .embed_cache
            .store(text, prefix, vec![value; EMBEDDING_DIM]);
    }

    async fn seed(engine: &Engine, text: &str, metadata: &str, value: f32) -> i64 {
        let state = engine.state.lock().await;
        match state
            .db
            .store_revalidated(
                &[Some(vec![value; EMBEDDING_DIM])],
                &[None],
                &[text.into()],
                "doc",
                &[metadata.into()],
            )
            .unwrap()
        {
            StoreAttempt::Committed { ids, .. } => ids[0],
            _ => panic!("seed deferred"),
        }
    }

    #[tokio::test]
    async fn superseded_filter_precedes_top_k_and_retrieval_count_increments() {
        let (engine, dir) = engine("superseded-search");
        let old = seed(&engine, "old", "{}", 0.0).await;
        let active = seed(&engine, "active", r#"{"domain":"mixos"}"#, 1.0).await;
        let other = seed(&engine, "other", r#"{"domain":"other"}"#, 0.5).await;
        assert_eq!(
            engine
                .supersede(json!({"old_id": old, "new_id": active, "reason": "newer"}))
                .await["updated"],
            1
        );
        cache(&engine, "query", "search_query: ", 0.0).await;
        let response = engine.search(json!({"query":"query", "limit":1, "metadata_filter":[{"field":"domain","op":"eq","value":"mixos"}]})).await;
        assert_eq!(response["results"].as_array().unwrap().len(), 1);
        assert_eq!(response["results"][0]["id"], active);
        assert!(response["results"][0]["metadata"].is_string());
        let response = engine.search(json!({"query":"query", "limit":10})).await;
        let ids: Vec<_> = response["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [other, active]);
        let state = engine.state.lock().await;
        let counts: Vec<(i64, i64)> = state
            .db
            .conn
            .prepare("SELECT id, retrieval_count FROM chunks ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(counts, [(old, 0), (active, 2), (other, 1)]);
        drop(state);
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn json_verbs_keep_metadata_strings_and_do_not_require_a_model_for_reads() {
        let (engine, dir) = engine("json-contract");
        let metadata = r#"{"path":"/srv/project/a.md","section":"A","domain":"mixos","type":"doc","date":"2026-10-20"}"#;
        cache(&engine, "text", "search_document: ", 0.0).await;
        let stored = engine
            .store(json!({"texts":["text"], "metadata":[metadata]}))
            .await;
        let id = stored["ids"][0].as_i64().unwrap();
        assert_eq!(stored["stored"], 1);
        let duplicate = engine
            .store(json!({"texts":["text"], "metadata":[metadata]}))
            .await;
        assert_eq!(duplicate["duplicates"], 1);
        assert_eq!(duplicate["ids"][0], id);
        let listed = engine.list(json!({"limit":10,"offset":0})).await;
        assert_eq!(listed["total"], 1);
        assert_eq!(listed["items"][0]["metadata"], metadata);
        assert!(listed["items"][0]["created"].is_string());
        assert_eq!(
            engine
                .update(json!({"id":id,"metadata":"{\"new\":true}"}))
                .await,
            json!({"updated":true,"re_embedded":false})
        );
        cache(&engine, "changed", "search_document: ", 0.2).await;
        assert_eq!(
            engine.update(json!({"id":id,"content":"changed"})).await,
            json!({"updated":true,"re_embedded":true})
        );
        assert_eq!(
            engine.feedback(json!({"id":id,"useful":true})).await["feedback_score"],
            1
        );
        let stale = engine.stale(json!({"never_retrieved_age_days":-1,"low_value_min_retrievals":3,"long_dormant_days":180,"per_bucket_limit":10})).await;
        assert_eq!(stale["total_chunks"], 1);
        assert_eq!(stale["never_retrieved_old"][0]["id"], id);
        assert!(stale["low_value"].is_array());
        assert!(stale["long_dormant"].is_array());
        let stats = engine.stats(json!({})).await;
        assert_eq!(stats["total_vectors"], 1);
        assert_eq!(stats["model_loaded"], false);
        assert!(stats["db_size_bytes"].is_u64());
        assert!(stats["by_source"].is_array());
        assert_eq!(
            engine.embed(json!({"texts":["text"]})).await["embeddings"][0]
                .as_array()
                .unwrap()
                .len(),
            EMBEDDING_DIM
        );
        assert_eq!(
            engine.delete(json!({"ids":[id]})).await,
            json!({"deleted":1})
        );
        assert!(engine.store(json!({"texts":[],"metadata":[{}]})).await["error"].is_string());
        assert_eq!(engine.stats(json!({})).await["total_vectors"], 0);
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn index_file_keeps_old_chunks_if_new_chunks_fail() {
        let (engine, dir) = engine("index-failure");
        let metadata = r#"{"path":"/srv/project/file.md","date":"2026-10-20"}"#;
        let id = seed(&engine, "previous content", metadata, 0.0).await;
        let response = engine
            .index_file(json!({"path":"/srv/project/file.md","content":"new content ".repeat(20)}))
            .await;
        assert!(
            response["error"].is_string(),
            "missing local tokenizer must fail: {response}"
        );
        let listed = engine.list(json!({})).await;
        assert_eq!(listed["items"][0]["id"], id);
        assert_eq!(listed["items"][0]["content"], "previous content");
        assert_eq!(
            engine
                .index_file(json!({"path":"/srv/project/file.md","content":""}))
                .await["sections"],
            0
        );
        assert_eq!(engine.list(json!({})).await["total"], 0);
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn index_file_replaces_old_chunks_after_all_sections_store() {
        let dir = temp_dir("index-success");
        let mut config = Config::default();
        config.service.vectors_db = dir.join("vectors.db").to_string_lossy().into_owned();
        config.source_types.insert(
            "journal".into(),
            SourceTypeSpec {
                required: vec!["path".into(), "domain".into(), "date".into()],
                date_field: Some("date".into()),
            },
        );
        config
            .domains
            .map
            .insert("/srv/project".into(), "parent".into());
        config
            .domains
            .map
            .insert("/srv/project/deep".into(), "child".into());
        let engine = Engine::open(config).unwrap();
        let tokenizer = tokenizers::models::wordlevel::WordLevel::builder()
            .vocab([("[UNK]".to_string(), 0)].into_iter().collect())
            .unk_token("[UNK]".into())
            .build()
            .unwrap();
        engine.state.lock().await.chunk_tokenizer = Some(Arc::new(ChunkTokenizer {
            tokenizer: Tokenizer::new(tokenizer),
            max_seq_len: 2048,
        }));
        let path = "/srv/project/deep/_journal/2026-10-20-entry.md";
        let old = seed(&engine, "obsolete", &json!({"path":path}).to_string(), 1.0).await;
        let first = format!("## First\n{}", "a".repeat(100));
        let second = format!("## Second\n{}", "b".repeat(100));
        cache(&engine, &first, "search_document: ", 0.0).await;
        cache(&engine, &second, "search_document: ", 0.5).await;
        let body = json!({"path":path, "content":format!("{first}\n{second}")});
        let result = engine.index_file(body.clone()).await;
        assert_eq!(result["sections"], 2, "{result}");
        assert_eq!(result["domain"], "child");
        let listed = engine.list(json!({})).await;
        assert_eq!(listed["total"], 2);
        for item in listed["items"].as_array().unwrap() {
            assert_ne!(item["id"], old);
            assert_eq!(item["source"], "journal");
            let metadata: serde_json::Value =
                serde_json::from_str(item["metadata"].as_str().unwrap()).unwrap();
            assert_eq!(metadata["date"], "2026-10-20");
            assert_eq!(metadata["domain"], "child");
            assert_eq!(metadata["path"], path);
        }
        assert_eq!(engine.index_file(body).await["sections"], 2);
        assert_eq!(
            engine.list(json!({})).await,
            listed,
            "unchanged reindex preserves IDs and metadata"
        );
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
