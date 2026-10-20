// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

// --- Circuit breaker for model loading ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CircuitState {
    Closed,
    Open { opened_at: Instant },
    HalfOpen,
}

pub(crate) struct CircuitBreaker {
    pub(crate) state: CircuitState,
    pub(crate) consecutive_failures: u32,
    pub(crate) failure_threshold: u32,
    pub(crate) cooldown: Duration,
}

impl CircuitBreaker {
    pub(crate) fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            failure_threshold,
            cooldown,
        }
    }

    pub(crate) fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open { opened_at } => {
                if opened_at.elapsed() >= self.cooldown {
                    self.state = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
    }

    pub(crate) fn record_failure(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.failure_threshold {
                    warn!(
                        "model circuit breaker OPEN after {} consecutive failures",
                        self.consecutive_failures
                    );
                    self.state = CircuitState::Open {
                        opened_at: Instant::now(),
                    };
                }
            }
            CircuitState::HalfOpen => {
                warn!("model circuit breaker re-OPEN (half-open probe failed)");
                self.state = CircuitState::Open {
                    opened_at: Instant::now(),
                };
            }
            CircuitState::Open { .. } => {}
        }
    }

    pub(crate) fn state_name(&self) -> &'static str {
        match self.state {
            CircuitState::Closed => "closed",
            CircuitState::Open { .. } => "open",
            CircuitState::HalfOpen => "half-open",
        }
    }
}

// --- Embedding cache (FNV-1a keyed by text+prefix, TTL eviction) ---

pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
pub(crate) const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
pub(crate) const EMBED_CACHE_TTL_SECS: u64 = 300; // 5 minutes
pub(crate) const EMBED_CACHE_MAX_ENTRIES: usize = 512;

pub(crate) struct CachedEmbedding {
    pub(crate) embedding: Vec<f32>,
    pub(crate) created_at: Instant,
}

pub(crate) struct EmbeddingCache {
    pub(crate) entries: HashMap<u64, CachedEmbedding>,
    pub(crate) ttl: Duration,
    pub(crate) max_entries: usize,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
}

impl EmbeddingCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ttl: Duration::from_secs(EMBED_CACHE_TTL_SECS),
            max_entries: EMBED_CACHE_MAX_ENTRIES,
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn lookup(&mut self, text: &str, prefix: &str) -> Option<Vec<f32>> {
        let key = fnv1a_hash(text, prefix);
        if let Some(entry) = self.entries.get(&key) {
            if entry.created_at.elapsed() < self.ttl {
                self.hits += 1;
                return Some(entry.embedding.clone());
            }
            self.entries.remove(&key);
        }
        self.misses += 1;
        None
    }

    pub(crate) fn store(&mut self, text: &str, prefix: &str, embedding: Vec<f32>) {
        if self.entries.len() >= self.max_entries {
            // Evict oldest entry
            if let Some(&oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k)
            {
                self.entries.remove(&oldest_key);
            }
        }
        let key = fnv1a_hash(text, prefix);
        self.entries.insert(
            key,
            CachedEmbedding {
                embedding,
                created_at: Instant::now(),
            },
        );
    }

    /// Look up a batch, returning cached embeddings and indices that need computing.
    pub(crate) fn lookup_batch(
        &mut self,
        texts: &[String],
        prefix: &str,
    ) -> (Vec<Option<Vec<f32>>>, Vec<usize>) {
        let mut results = Vec::with_capacity(texts.len());
        let mut needs_embed = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            match self.lookup(text, prefix) {
                Some(emb) => results.push(Some(emb)),
                None => {
                    results.push(None);
                    needs_embed.push(i);
                }
            }
        }
        (results, needs_embed)
    }
}

pub(crate) fn fnv1a_hash(text: &str, prefix: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in prefix.bytes().chain(text.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// --- Content hash for deduplication ---
// --- Embedding model ---

pub(crate) struct EmbedModel {
    pub(crate) model: NomicBertModel,
    pub(crate) tokenizer: Tokenizer,
    pub(crate) device: Device,
    /// Rotary-position ceiling (`config.n_positions`). The candle nomic-bert
    /// rope tables are precomputed for exactly this many positions; feeding a
    /// longer token sequence makes `candle_nn::rotary_emb::rope` bail with
    /// "inconsistent last dim size in rope". `embed()` truncates to this as a
    /// hard backstop so an oversized input can never crash an embed call.
    pub(crate) max_seq_len: usize,
    /// Maximum `batch * padded_sequence_length` passed to one forward.
    pub(crate) max_forward_tokens: usize,
}

impl EmbedModel {
    pub(crate) fn load(
        dtype: DType,
        model_dir: &str,
        configured_max_seq_len: usize,
        configured_forward_tokens: usize,
    ) -> Result<Self> {
        let device = Device::Cpu;

        let model_dir = Path::new(model_dir);
        let config_path = model_dir.join("config.json");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let weights_path = model_dir.join("model.safetensors");
        for path in [&config_path, &tokenizer_path, &weights_path] {
            anyhow::ensure!(
                path.is_file(),
                "missing local model file {}",
                path.display()
            );
        }
        info!("loading model with {dtype:?} precision...");
        let config: nomic_bert::Config = serde_json::from_str(
            &std::fs::read_to_string(&config_path).context("reading config.json")?,
        )?;
        let mut tokenizer =
            Tokenizer::from_file(&tokenizer_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let max_seq_len = effective_sequence_limit(
            configured_max_seq_len,
            configured_forward_tokens,
            config.n_positions,
        )?;
        if max_seq_len != configured_max_seq_len {
            warn!(
                configured = configured_max_seq_len,
                effective = max_seq_len,
                n_positions = config.n_positions,
                max_forward_tokens = configured_forward_tokens,
                "lowered embedding sequence length to model/operator ceilings"
            );
        }
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: max_seq_len,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("configuring tokenizer truncation: {e}"))?;
        let max_forward_tokens = configured_forward_tokens;

        // F16 service loads always mmap an already-converted artefact. The
        // one-time conversion is completed and atomically renamed before the
        // model builder starts, so normal cold loads never retain the 522 MiB
        // F32 mapping alongside the resident F16 model.
        let runtime_weights = if dtype == DType::F16 {
            match ensure_f16_weights(&weights_path) {
                Ok(path) => path,
                Err(e) => {
                    warn!(
                        source = %weights_path.display(),
                        "f16 artefact unavailable ({e:#}); falling back to mmap F32 weights with runtime F16 casts"
                    );
                    weights_path
                }
            }
        } else {
            weights_path
        };
        // SAFETY: local weight files remain immutable for the lifetime of the model mapping.
        let vb =
            unsafe { VarBuilder::from_mmaped_safetensors(&[runtime_weights], dtype, &device)? };
        let model = NomicBertModel::load(vb, &config)?;

        info!("model loaded successfully");
        Ok(Self {
            model,
            tokenizer,
            device,
            max_seq_len,
            max_forward_tokens,
        })
    }

    pub(crate) fn embed(&self, texts: &[String], prefix: &str) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let prefixed: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();

        let mut tokens = self
            .tokenizer
            .encode_batch(
                prefixed.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                true,
            )
            .map_err(|e| anyhow::anyhow!("tokenization: {e}"))?;

        // Tokenizer truncation is configuration, not a memory-safety boundary.
        // Retain a release-mode backstop so a tokenizer/version regression can
        // never feed more positions than the model's rope tables hold.
        for encoding in &mut tokens {
            if encoding.get_ids().len() > self.max_seq_len {
                encoding.truncate(self.max_seq_len, 0, tokenizers::TruncationDirection::Right);
            }
        }
        let lengths: Vec<usize> = tokens.iter().map(|t| t.get_ids().len()).collect();
        let batches = plan_micro_batches(&lengths, self.max_forward_tokens);
        let mut results: Vec<Option<Vec<f32>>> = vec![None; tokens.len()];

        for batch in batches {
            let max_len = batch
                .iter()
                .map(|&i| tokens[i].get_ids().len())
                .max()
                .unwrap_or(0);
            let batch_size = batch.len();
            let mut all_ids = Vec::with_capacity(batch_size * max_len);
            let mut all_mask = Vec::with_capacity(batch_size * max_len);
            let mut all_type_ids = Vec::with_capacity(batch_size * max_len);

            for &original_idx in &batch {
                let encoding = &tokens[original_idx];
                let pad_len = max_len - encoding.get_ids().len();
                all_ids.extend_from_slice(encoding.get_ids());
                all_ids.extend(std::iter::repeat_n(0u32, pad_len));
                all_mask.extend_from_slice(encoding.get_attention_mask());
                all_mask.extend(std::iter::repeat_n(0u32, pad_len));
                all_type_ids.extend_from_slice(encoding.get_type_ids());
                all_type_ids.extend(std::iter::repeat_n(0u32, pad_len));
            }

            let input_ids = Tensor::from_vec(all_ids, (batch_size, max_len), &self.device)?;
            let attention_mask = Tensor::from_vec(all_mask, (batch_size, max_len), &self.device)?;
            let token_type_ids =
                Tensor::from_vec(all_type_ids, (batch_size, max_len), &self.device)?;
            let hidden =
                self.model
                    .forward(&input_ids, Some(&token_type_ids), Some(&attention_mask))?;
            let hidden = hidden.to_dtype(DType::F32)?;
            let pooled = nomic_bert::mean_pooling(&hidden, &attention_mask)?;
            let normalized = nomic_bert::l2_normalize(&pooled)?;
            for (batch_idx, &original_idx) in batch.iter().enumerate() {
                results[original_idx] = Some(normalized.get(batch_idx)?.to_vec1::<f32>()?);
            }
        }
        results
            .into_iter()
            .map(|value| value.ok_or_else(|| anyhow::anyhow!("missing micro-batch output")))
            .collect()
    }
}

pub(crate) fn effective_sequence_limit(
    configured: usize,
    max_forward_tokens: usize,
    n_positions: usize,
) -> Result<usize> {
    const MIN_SEQUENCE_TOKENS: usize = 16;
    const TRAINED_MAX_SEQUENCE_TOKENS: usize = 2048;
    if n_positions < MIN_SEQUENCE_TOKENS {
        anyhow::bail!(
            "model declares n_positions={n_positions}, below the minimum supported {MIN_SEQUENCE_TOKENS}"
        );
    }
    let configured = configured.clamp(MIN_SEQUENCE_TOKENS, TRAINED_MAX_SEQUENCE_TOKENS);
    let effective = configured
        .min(n_positions.min(TRAINED_MAX_SEQUENCE_TOKENS))
        .min(max_forward_tokens);
    if effective == 0 {
        anyhow::bail!("max_forward_tokens must be at least 1");
    }
    Ok(effective)
}

/// Length-aware ascending grouping minimises padding while keeping every
/// forward at or below `budget` padded tokens. Indices let `embed()` restore
/// caller order after the sorted execution plan.
pub(crate) fn plan_micro_batches(lengths: &[usize], budget: usize) -> Vec<Vec<usize>> {
    let mut ordered: Vec<usize> = (0..lengths.len()).collect();
    ordered.sort_by_key(|&i| lengths[i]);
    let budget = budget.max(1);
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_max = 0usize;
    for idx in ordered {
        let candidate_max = current_max.max(lengths[idx].max(1));
        let candidate_cost = candidate_max.saturating_mul(current.len() + 1);
        if !current.is_empty() && candidate_cost > budget {
            batches.push(std::mem::take(&mut current));
            current_max = 0;
        }
        current_max = current_max.max(lengths[idx].max(1));
        current.push(idx);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

pub(crate) fn ensure_f16_weights(weights_path: &Path) -> Result<PathBuf> {
    use candle_core::safetensors::Load;

    let target = weights_path.with_file_name("model.f16.safetensors");
    cleanup_f16_tmp_files(weights_path)?;
    if target.is_file() {
        match validate_f16_weights(weights_path, &target) {
            Ok(()) => return Ok(target),
            Err(e) => {
                warn!(target = %target.display(), "discarding invalid f16 artefact: {e:#}");
                std::fs::remove_file(&target)
                    .with_context(|| format!("removing invalid {}", target.display()))?;
            }
        }
    }
    let tmp =
        weights_path.with_file_name(format!(".model.f16.safetensors.tmp-{}", std::process::id()));
    info!(
        source = %weights_path.display(),
        target = %target.display(),
        "converting cached model weights to f16 (one time)"
    );
    // SAFETY: the source weight file remains immutable while this mapping is alive.
    let mapped = unsafe { candle_core::safetensors::MmapedSafetensors::new(weights_path)? };
    let mut converted = HashMap::new();
    for (name, view) in mapped.tensors() {
        if DType::try_from(view.dtype())? != DType::F32 {
            anyhow::bail!(
                "weight {name} is {:?}, expected all-F32 source",
                view.dtype()
            );
        }
        converted.insert(name, view.load(&Device::Cpu)?.to_dtype(DType::F16)?);
    }
    let install = (|| -> Result<PathBuf> {
        candle_core::safetensors::save(&converted, &tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        validate_f16_weights(weights_path, &tmp)?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, &target)
            .with_context(|| format!("installing {}", target.display()))?;
        if let Some(parent) = target.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        validate_f16_weights(weights_path, &target)?;
        Ok(target)
    })();
    if install.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    install
}

pub(crate) fn cleanup_f16_tmp_files(weights_path: &Path) -> Result<()> {
    const STALE_AFTER: Duration = Duration::from_secs(60 * 60);
    let Some(parent) = weights_path.parent() else {
        return Ok(());
    };
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if let Some(name) = file_name
            .to_str()
            .filter(|name| name.starts_with(".model.f16.safetensors.tmp-"))
        {
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
            let owner_dead = name
                .strip_prefix(".model.f16.safetensors.tmp-")
                .and_then(|pid| pid.parse::<u32>().ok())
                .is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists());
            if !stale && !owner_dead {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => {
                    tracing::debug!(path = %entry.path().display(), "removed stale f16 conversion temporary")
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("removing {}", entry.path().display()));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_f16_weights(source: &Path, target: &Path) -> Result<()> {
    // SAFETY: both weight files remain immutable while their mappings are alive.
    let source = unsafe { candle_core::safetensors::MmapedSafetensors::new(source)? };
    // SAFETY: the target stays immutable until validation drops this mapping.
    let target = unsafe { candle_core::safetensors::MmapedSafetensors::new(target)? };
    let source_tensors = source.tensors();
    let target_tensors = target.tensors();
    if source_tensors.len() != target_tensors.len() {
        anyhow::bail!(
            "f16 tensor count {} != source count {}",
            target_tensors.len(),
            source_tensors.len()
        );
    }
    let target_by_name: HashMap<_, _> = target_tensors.into_iter().collect();
    for (name, source_view) in source_tensors {
        let target_view = target_by_name
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("f16 artefact missing tensor {name}"))?;
        if DType::try_from(target_view.dtype())? != DType::F16 {
            anyhow::bail!("f16 tensor {name} has dtype {:?}", target_view.dtype());
        }
        if target_view.shape() != source_view.shape() {
            anyhow::bail!(
                "f16 tensor {name} shape {:?} != source {:?}",
                target_view.shape(),
                source_view.shape()
            );
        }
    }
    Ok(())
}
/// one reference until its request-owned model Arc is dropped; the blocking
/// worker keeps another until the forward really exits (including timeout or
/// panic). The permit therefore outlives every model reference from the request.
pub(crate) struct InferenceLease {
    pub(crate) _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Atomic lifecycle for a blocking inference: 0=running, 1=caller timed out,
/// 2=worker finished. This closes the timeout/finish race while keeping the
/// timed-out-still-running gauge exact.
pub(crate) struct InferenceWorkerGuard {
    pub(crate) _lease: Arc<InferenceLease>,
    pub(crate) lifecycle: Arc<AtomicU8>,
}

impl Drop for InferenceWorkerGuard {
    fn drop(&mut self) {
        metrics::INFERENCE_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        if self.lifecycle.swap(2, Ordering::AcqRel) == 1 {
            metrics::TIMED_OUT_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn mark_inference_timed_out(lifecycle: &AtomicU8) {
    if lifecycle
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        metrics::TIMED_OUT_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn rss_bytes() -> u64 {
    let resident_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0);
    // SAFETY: sysconf takes a constant selector and no pointers.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        0
    } else {
        resident_pages.saturating_mul(page_size as u64)
    }
}

/// Background indexing job envelope. Carries the origin context so the
/// worker's CPU is attributed to the requester that queued it — the
/// foreground `index_file` ack is near-zero cost, the real work lands
pub(crate) async fn ensure_model(
    state: &Arc<EngineState>,
    lease: Arc<InferenceLease>,
    t: &mut ReqTiming,
) -> Result<Arc<EmbedModel>> {
    let mut guard = lock_timed(state, t).await;
    if let Some(model) = &guard.model {
        return Ok(model.clone());
    }
    if !guard.model_breaker.allow_request() {
        anyhow::bail!(
            "model loading suspended (circuit {}, cooldown active)",
            guard.model_breaker.state_name()
        );
    }
    info!("loading model on demand...");
    let dtype = guard.dtype;
    drop(guard);
    let svc = state.config.service.clone();
    // The worker retains the permit even if the requesting future is cancelled.
    let loaded = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        metrics::RSS_BEFORE_LOAD_BYTES.store(rss_bytes(), Ordering::Relaxed);
        let model = EmbedModel::load(
            dtype,
            &svc.model_dir,
            svc.max_sequence_tokens,
            svc.max_forward_tokens,
        );
        metrics::RSS_AFTER_LOAD_BYTES.store(rss_bytes(), Ordering::Relaxed);
        model
    })
    .await
    .context("model loader worker failed")?;
    let mut guard = lock_timed(state, t).await;
    match loaded {
        Ok(model) => {
            let model = Arc::new(model);
            guard.model = Some(model.clone());
            guard.model_breaker.record_success();
            metrics::MODEL_GENERATION.fetch_add(1, Ordering::Relaxed);
            Ok(model)
        }
        Err(e) => {
            guard.model_breaker.record_failure();
            Err(e)
        }
    }
}

/// Compute embeddings for `texts` (with `prefix`) WITHOUT holding the
/// global `AppState` lock across the model forward pass. This is the core
/// concurrency fix: a slow embed must not block reads, search, or health.
///
/// Lock discipline (short critical sections, never across the embed):
///  1. lock → cache lookup and clone the inference gate, then drop lock.
///  2. acquire the inference permit OFF-lock, then re-lock/recheck the cache,
///     load/clone the model while the permit protects it from idle unload.
///  3. (no lock) run `model.embed` on a blocking thread under a wall-clock
///     timeout; on any error/timeout re-lock just to record the breaker
///     failure.
///  4. lock → record breaker success, store fresh embeddings into the cache,
///     fill the result vec, drop lock.
///
/// Returns the embeddings for `texts` in order. Errors are human-readable
/// strings the callers wrap into `json_error`.
pub(crate) async fn compute_embeddings(
    state: &Arc<EngineState>,
    texts: &[String],
    prefix: &str,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    let config = state.config.clone();
    validate_embedding_budget(texts, prefix, &config.service)?;

    // Cache fast path, then permit-before-model ordering. The second cache
    // lookup is required because another admitted request may populate the
    // misses while this request waits for the permit.
    let inference_gate = {
        let mut guard = lock_timed(state, t).await;
        let (cached, needs_embed) = guard.embed_cache.lookup_batch(texts, prefix);
        if needs_embed.is_empty() {
            t.cache_hit = true;
            return Ok(cached.into_iter().map(|o| o.unwrap()).collect());
        }
        guard.inference_gate.clone()
    };

    let permit = acquire_inference_permit(
        inference_gate,
        Duration::from_secs(config.service.inference_admission_timeout_secs.max(1)),
    )
    .await?;

    let inference_lease = Arc::new(InferenceLease { _permit: permit });
    let (mut cached, needs_embed, was_cold) = {
        let mut guard = lock_timed(state, t).await;
        let (cached, needs_embed) = guard.embed_cache.lookup_batch(texts, prefix);
        if needs_embed.is_empty() {
            t.cache_hit = true;
            return Ok(cached.into_iter().map(|o| o.unwrap()).collect());
        }

        if !guard.embed_breaker.allow_request() {
            return Err(format!(
                "embed circuit open (state {}, cooldown active)",
                guard.embed_breaker.state_name()
            ));
        }

        (cached, needs_embed, guard.model.is_none())
    };
    let load_t0 = Instant::now();
    let load_res = ensure_model(state, inference_lease.clone(), t).await;
    if was_cold {
        // Commit BEFORE checking the result: a slow FAILED load is
        // exactly the burn this instrumentation must expose, not
        // unexplained elapsed time with cold_model=false.
        t.cold_model = true;
        t.model_load_us += load_t0.elapsed().as_micros() as u64;
    }
    let model = load_res.map_err(|e| format!("model load failed: {e}"))?;

    let texts_to_embed: Vec<String> = needs_embed.iter().map(|&i| texts[i].clone()).collect();

    // Reset the idle watchdog clock now that we're about to do real work.
    let _ = activity_tx.send(()).await;

    // --- Off-lock embed on a blocking thread, under a wall-clock timeout ---
    metrics::INFERENCE_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    let lifecycle = Arc::new(AtomicU8::new(0));
    let worker_lifecycle = lifecycle.clone();
    let worker = InferenceWorkerGuard {
        _lease: inference_lease.clone(),
        lifecycle: worker_lifecycle,
    };
    let model2 = model.clone();
    let to_embed = texts_to_embed.clone();
    let pfx = prefix.to_string();
    let embed_t0 = Instant::now();
    let res = tokio::time::timeout(
        Duration::from_secs(EMBED_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            let _worker = worker;
            model2.embed(&to_embed, &pfx)
        }),
    )
    .await;
    t.embed_us += embed_t0.elapsed().as_micros() as u64;

    let new_embeddings: Vec<Vec<f32>> = match res {
        Ok(Ok(Ok(v))) => v,
        Ok(Ok(Err(e))) => {
            record_embed_failure(state, t).await;
            drop(model);
            drop(inference_lease);
            return Err(format!("embedding failed: {e}"));
        }
        Ok(Err(join)) => {
            record_embed_failure(state, t).await;
            drop(model);
            drop(inference_lease);
            return Err(format!("embedding worker panicked: {join}"));
        }
        Err(_elapsed) => {
            mark_inference_timed_out(&lifecycle);
            record_embed_failure(state, t).await;
            drop(model);
            drop(inference_lease);
            return Err(format!("embedding timed out after {EMBED_TIMEOUT_SECS}s"));
        }
    };

    // The model must return exactly one embedding per requested text. A short
    // result would otherwise panic the indexing below; treat it as a failure
    // (record + return) rather than recording success and indexing out of
    // bounds. This also keeps the empty-result case a clean error for callers.
    if new_embeddings.len() != needs_embed.len() {
        record_embed_failure(state, t).await;
        drop(model);
        drop(inference_lease);
        return Err(format!(
            "embedding failed: model returned {} embeddings, expected {}",
            new_embeddings.len(),
            needs_embed.len()
        ));
    }

    // --- Short lock #2: record success + cache store + fill results ---
    {
        let mut guard = lock_timed(state, t).await;
        guard.embed_breaker.record_success();
        for (embed_idx, &original_idx) in needs_embed.iter().enumerate() {
            let emb = new_embeddings[embed_idx].clone();
            guard
                .embed_cache
                .store(&texts[original_idx], prefix, emb.clone());
            cached[original_idx] = Some(emb);
        }
    }

    let embeddings = cached.into_iter().map(|o| o.unwrap()).collect();
    drop(model);
    drop(inference_lease);
    Ok(embeddings)
}

pub(crate) async fn acquire_inference_permit(
    gate: Arc<tokio::sync::Semaphore>,
    deadline: Duration,
) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    match tokio::time::timeout(deadline, gate.acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) => Err("inference_busy: inference gate closed".to_string()),
        Err(_) => Err(format!(
            "inference_busy: inference permit unavailable after {}s",
            deadline.as_secs()
        )),
    }
}

/// Re-lock state solely to record an embed-breaker failure (error/timeout).
/// The re-lock wait is charged to the failing request's timing — failed
/// embeds must not under-report lock contention.
pub(crate) async fn record_embed_failure(state: &Arc<EngineState>, t: &mut ReqTiming) {
    lock_timed(state, t).await.embed_breaker.record_failure();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;
    #[test]
    fn micro_batches_stay_in_budget_and_cover_original_order() {
        assert_eq!(effective_sequence_limit(2048, 4096, 8192).unwrap(), 2048);
        assert_eq!(effective_sequence_limit(16_384, 4096, 8192).unwrap(), 2048);
        assert_eq!(effective_sequence_limit(0, 4096, 8192).unwrap(), 16);
        assert_eq!(
            effective_sequence_limit(2048, 512, 8192).unwrap(),
            512,
            "operator forward ceiling lowers the effective sequence limit"
        );
        assert!(
            effective_sequence_limit(2048, 4096, 8)
                .unwrap_err()
                .to_string()
                .contains("n_positions=8")
        );
        let lengths = [2048, 100, 401, 2048, 99, 400, 1000];
        let batches = plan_micro_batches(&lengths, 4096);
        let mut seen = Vec::new();
        for batch in &batches {
            let padded = batch.iter().map(|&i| lengths[i]).max().unwrap() * batch.len();
            assert!(padded <= 4096, "batch {batch:?} costs {padded}");
            seen.extend(batch.iter().copied());
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..lengths.len()).collect::<Vec<_>>());
    }
    #[tokio::test]
    async fn inference_permit_wait_is_bounded_and_clear() {
        let gate = Arc::new(tokio::sync::Semaphore::new(1));
        let _held = gate.clone().acquire_owned().await.unwrap();
        let error = acquire_inference_permit(gate, Duration::from_millis(5))
            .await
            .unwrap_err();
        assert!(error.starts_with("inference_busy:"), "{error}");
    }

    #[tokio::test]
    async fn inference_lease_holds_permit_until_caller_and_worker_release() {
        let gate = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = gate.clone().acquire_owned().await.unwrap();
        let caller = Arc::new(InferenceLease { _permit: permit });
        let worker = caller.clone();
        drop(caller);
        assert_eq!(gate.available_permits(), 0);
        drop(worker);
        assert_eq!(gate.available_permits(), 1);
    }
    #[test]
    fn f16_safetensor_conversion_preserves_embedding_cosine() {
        let dir = temp_dir("f16-conversion");
        let source = dir.join("model.safetensors");
        let values = vec![0.125f32, -1.75, 2.345_678, 0.000_123];
        let tensor = Tensor::from_vec(values, (4,), &Device::Cpu).unwrap();
        let direct = tensor
            .to_dtype(DType::F16)
            .unwrap()
            .to_dtype(DType::F32)
            .unwrap()
            .to_vec1::<f32>()
            .unwrap();
        let mut tensors = HashMap::new();
        tensors.insert("embedding".to_string(), tensor);
        candle_core::safetensors::save(&tensors, &source).unwrap();

        let converted = ensure_f16_weights(&source).unwrap();
        let got = candle_core::safetensors::load(&converted, &Device::Cpu)
            .unwrap()
            .remove("embedding")
            .unwrap()
            .to_dtype(DType::F32)
            .unwrap()
            .to_vec1::<f32>()
            .unwrap();
        let dot: f32 = direct.iter().zip(&got).map(|(a, b)| a * b).sum();
        let norm_a: f32 = direct.iter().map(|v| v * v).sum::<f32>().sqrt();
        let norm_b: f32 = got.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(dot / (norm_a * norm_b) >= 0.999);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f16_conversion_cleans_tmp_and_replaces_invalid_target() {
        let dir = temp_dir("f16-recovery");
        let source = dir.join("model.safetensors");
        let tensor = Tensor::from_vec(vec![1.0f32, 2.0], (2,), &Device::Cpu).unwrap();
        let mut tensors = HashMap::new();
        tensors.insert("expected".to_string(), tensor);
        candle_core::safetensors::save(&tensors, &source).unwrap();
        let stale = dir.join(".model.f16.safetensors.tmp-4294967295");
        std::fs::write(&stale, b"stale").unwrap();
        let target = dir.join("model.f16.safetensors");
        std::fs::write(&target, b"not safetensors").unwrap();

        let converted = ensure_f16_weights(&source).unwrap();
        assert_eq!(converted, target);
        assert!(!stale.exists());
        validate_f16_weights(&source, &target).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
