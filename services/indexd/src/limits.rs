// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

pub(crate) const EMBEDDING_DIM: usize = 768;

/// Wall-clock cap on a single off-lock embed batch. A hung candle forward
/// pass must not pin the embed circuit breaker open forever; on expiry the
/// The request returns an error on expiry, but the blocking worker retains its
/// inference permit until the OS thread really exits.
pub(crate) const EMBED_TIMEOUT_SECS: u64 = 120;

/// Max texts accepted in a single embed/store request. Token/byte budgets are
/// enforced separately; count alone is not a memory bound.
pub(crate) const MAX_TEXTS_PER_REQUEST: usize = 256;

/// Max metadata filters in a single search. Each filter adds a bound SQL
/// clause; an unbounded array forces O(n) query-string assembly + param
/// boxing. A real query uses a handful.
pub(crate) const MAX_METADATA_FILTERS: usize = 32;

/// Max sections one `index_file` request may produce after splitting
/// caller-supplied `content`. Each section is embedded sequentially, so
/// without this a single request (foreground OR background) can
/// monopolise the embedding worker indefinitely — bypassing
/// `MAX_TEXTS_PER_REQUEST` because each internal store is a 1-element
/// batch. Generous for a large real document (≈ content_bytes /
/// MAX_CHUNK_CHARS), low enough to reject a flood of tiny sections.
pub(crate) const MAX_INDEX_FILE_SECTIONS: usize = 4000;

pub(crate) fn transient_background_failure(response: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
        .is_some_and(|error| {
            error.contains("inference_busy:")
                || error.contains("model loading suspended")
                || error.contains("embed circuit open")
        })
}

pub(crate) fn background_retry_backoff(initial_secs: u64, failed_attempt: usize) -> Duration {
    let mut backoff = Duration::from_secs(initial_secs.clamp(1, 60));
    for _ in 1..failed_attempt {
        if backoff >= Duration::from_secs(60) {
            break;
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
    backoff
}
pub(crate) const MAX_LIST_LIMIT: usize = 1000;
pub(crate) const MAX_STALE_BUCKET_LIMIT: usize = 200;

pub(crate) fn clamp_list_limit(limit: usize) -> usize {
    limit.min(MAX_LIST_LIMIT)
}

pub(crate) fn clamp_stale_limit(limit: usize) -> usize {
    limit.min(MAX_STALE_BUCKET_LIMIT)
}

pub(crate) fn sqlite_count(value: usize, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds SQLite integer range"))
}

pub(crate) fn charge_response_item(
    used: &mut usize,
    fields: &[&str],
    max_bytes: usize,
) -> Result<()> {
    let item_bytes = fields.iter().try_fold(256usize, |total, field| {
        let serialised_len = serde_json::to_string(field)?.len();
        total
            .checked_add(serialised_len)
            .ok_or_else(|| anyhow::anyhow!("response byte count overflow"))
    })?;
    let next = used
        .checked_add(item_bytes)
        .ok_or_else(|| anyhow::anyhow!("response byte count overflow"))?;
    if next > max_bytes {
        anyhow::bail!("full-content response exceeds configured max {max_bytes} bytes");
    }
    *used = next;
    Ok(())
}

pub(crate) fn total_text_bytes(texts: &[String]) -> Option<usize> {
    texts
        .iter()
        .try_fold(0usize, |total, text| total.checked_add(text.len()))
}

pub(crate) fn total_embedding_input_bytes(texts: &[String], prefix: &str) -> Option<usize> {
    total_text_bytes(texts)?.checked_add(prefix.len().checked_mul(texts.len())?)
}

pub(crate) fn validate_embedding_budget(
    texts: &[String],
    prefix: &str,
    service: &ServiceConfig,
) -> Result<(), String> {
    let max_prefix = service.max_prefix_bytes;
    if prefix.len() > max_prefix {
        return Err(format!(
            "prefix is {} bytes (max {max_prefix})",
            prefix.len()
        ));
    }
    let max = service.max_request_text_bytes;
    let bytes = total_embedding_input_bytes(texts, prefix)
        .ok_or_else(|| "embedding input byte count overflow".to_string())?;
    if bytes > max {
        Err(format!(
            "texts plus repeated prefixes total {bytes} bytes exceeds max {max}"
        ))
    } else {
        Ok(())
    }
}
pub(crate) struct IndexJob {
    pub(crate) req: IndexFileRequest,
    /// `REQUEST_SEQ` of the foreground request that queued this job.
    pub(crate) seq: u64,
    /// "transport:peer" of the original requester.
    pub(crate) peer: String,
    /// Byte size of the original request line.
    pub(crate) bytes: u64,
    pub(crate) enqueued: Instant,
    /// One-based inference/model attempt number for the next worker run.
    pub(crate) attempt: usize,
    pub(crate) reservation: QueueReservation,
}

#[derive(Default)]
pub(crate) struct QueueUsage {
    pub(crate) jobs: usize,
    pub(crate) bytes: usize,
}

pub(crate) struct QueueBudget {
    pub(crate) max_jobs: usize,
    pub(crate) max_bytes: usize,
    pub(crate) usage: std::sync::Mutex<QueueUsage>,
}

impl QueueBudget {
    pub(crate) fn new(max_jobs: usize, max_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            max_jobs,
            max_bytes,
            usage: std::sync::Mutex::new(QueueUsage::default()),
        })
    }

    pub(crate) fn try_reserve(self: &Arc<Self>, bytes: usize) -> Result<QueueReservation, ()> {
        let mut usage = self.usage.lock().expect("queue budget mutex poisoned");
        let next_bytes = usage.bytes.checked_add(bytes).ok_or(())?;
        if usage.jobs >= self.max_jobs || next_bytes > self.max_bytes {
            return Err(());
        }
        usage.jobs += 1;
        usage.bytes = next_bytes;
        metrics::QUEUE_DEPTH.store(usage.jobs, Ordering::Relaxed);
        metrics::QUEUED_BYTES.store(usage.bytes as u64, Ordering::Relaxed);
        Ok(QueueReservation {
            budget: self.clone(),
            bytes,
            active: true,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.usage.lock().expect("queue budget mutex poisoned").jobs == 0
    }
}

pub(crate) struct QueueReservation {
    pub(crate) budget: Arc<QueueBudget>,
    pub(crate) bytes: usize,
    pub(crate) active: bool,
}

impl QueueReservation {
    /// Returns true exactly when this release transitions queued jobs to zero.
    pub(crate) fn release(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        let mut usage = self
            .budget
            .usage
            .lock()
            .expect("queue budget mutex poisoned");
        usage.jobs -= 1;
        usage.bytes = usage.bytes.saturating_sub(self.bytes);
        metrics::QUEUE_DEPTH.store(usage.jobs, Ordering::Relaxed);
        metrics::QUEUED_BYTES.store(usage.bytes as u64, Ordering::Relaxed);
        usage.jobs == 0
    }
}

impl Drop for QueueReservation {
    fn drop(&mut self) {
        self.release();
    }
}

pub(crate) enum DeferredRequeueFailure {
    Closed(IndexJob),
    Full { job: IndexJob, attempts: usize },
}

pub(crate) async fn defer_background_job(
    mut job: IndexJob,
    job_tx: tokio::sync::mpsc::Sender<IndexJob>,
    initial_backoff: Duration,
    max_attempts: usize,
) -> Result<(), DeferredRequeueFailure> {
    let max_attempts = max_attempts.max(1);
    let mut backoff = initial_backoff;
    for requeue_attempt in 1..=max_attempts {
        tokio::time::sleep(backoff).await;
        match job_tx.try_send(job) {
            Ok(()) => return Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(returned)) => {
                return Err(DeferredRequeueFailure::Closed(returned));
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
                job = returned;
                if requeue_attempt == max_attempts {
                    return Err(DeferredRequeueFailure::Full {
                        job,
                        attempts: requeue_attempt,
                    });
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
                warn!(
                    seq = job.seq,
                    path = %job.req.path,
                    requeue_attempt,
                    retry_in_secs = backoff.as_secs(),
                    "indexd deferred background job still waiting for queue capacity"
                );
            }
        }
    }
    unreachable!("bounded deferred requeue loop always returns")
}

pub(crate) async fn release_background_reservation(
    job: &mut IndexJob,
    state: &Arc<EngineState>,
    queue_budget: &Arc<QueueBudget>,
) {
    let drained_at_completion = job.reservation.release();
    if drained_at_completion && queue_budget.is_empty() {
        if LONG_READERS.load(Ordering::Acquire) == 0 {
            let guard = state.lock().await;
            if let Err(e) = guard.db.checkpoint_truncate() {
                guard.db.mark_checkpoint_owed();
                tracing::debug!("background-drain WAL checkpoint skipped: {e}");
            }
        } else {
            state.lock().await.db.mark_checkpoint_owed();
        }
        // Bulk indexing can leave freed allocator spans resident; trim once
        // at the queue-empty transition, never per job.
        // SAFETY: malloc_trim accepts zero and only releases unused glibc pages.
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

pub(crate) fn index_job_owned_bytes(req: &IndexFileRequest) -> usize {
    req.path
        .len()
        .saturating_add(req.source.len())
        .saturating_add(req.domain.len())
        .saturating_add(req.content.as_ref().map_or(0, String::len))
}

pub(crate) static LONG_READERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(crate) struct LongReaderGuard;

impl LongReaderGuard {
    pub(crate) fn new() -> Self {
        LONG_READERS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for LongReaderGuard {
    fn drop(&mut self) {
        if LONG_READERS.fetch_sub(1, Ordering::AcqRel) == 1 {
            CHECKPOINT_RETRY_NOTIFY.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limit_clamps_and_sqlite_count_validation() {
        assert_eq!(clamp_list_limit(usize::MAX), 1000);
        assert_eq!(clamp_stale_limit(usize::MAX), 200);
        assert_eq!(sqlite_count(42, "limit").unwrap(), 42);
        #[cfg(target_pointer_width = "64")]
        assert!(sqlite_count(usize::MAX, "offset").is_err());
    }

    #[test]
    fn aggregate_text_and_job_byte_accounting() {
        let texts = vec!["abc".to_string(), "é".to_string()];
        assert_eq!(total_text_bytes(&texts), Some(5));
        assert_eq!(
            total_embedding_input_bytes(&texts, "prefix"),
            Some(5 + 2 * 6),
            "prefix bytes are charged once per text"
        );
        assert_eq!(
            total_embedding_input_bytes(&vec![String::new(); 256], &"x".repeat(4096)),
            Some(256 * 4096)
        );
        let req = IndexFileRequest {
            path: "p".repeat(3),
            content: Some("body".repeat(2)),
            source: "doc".into(),
            domain: "d".into(),
            background: true,
        };
        assert_eq!(index_job_owned_bytes(&req), 3 + 8 + 3 + 1);
    }

    #[test]
    fn queue_budget_enforces_bytes_and_count() {
        let budget = QueueBudget::new(2, 10);
        let mut first = budget.try_reserve(6).unwrap();
        assert!(budget.try_reserve(5).is_err());
        let second = budget.try_reserve(4).unwrap();
        assert!(budget.try_reserve(0).is_err());
        assert!(!first.release());
        drop(second);
        assert!(budget.is_empty());
        assert!(budget.try_reserve(11).is_err());
    }

    #[tokio::test]
    async fn deferred_requeue_holds_reservation_until_loud_abandon() {
        fn job(path: &str, budget: &Arc<QueueBudget>) -> IndexJob {
            IndexJob {
                req: IndexFileRequest {
                    path: path.to_string(),
                    content: None,
                    source: "doc".to_string(),
                    domain: "test".to_string(),
                    background: true,
                },
                seq: 1,
                peer: "test:peer".to_string(),
                bytes: path.len() as u64,
                enqueued: Instant::now(),
                attempt: 2,
                reservation: budget.try_reserve(path.len()).unwrap(),
            }
        }

        let budget = QueueBudget::new(2, 1024);
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        assert!(tx.try_send(job("queued", &budget)).is_ok());
        let deferred = job("deferred", &budget);
        let failure = defer_background_job(deferred, tx, Duration::ZERO, 2).await;
        let mut abandoned = match failure {
            Err(DeferredRequeueFailure::Full { job, attempts }) => {
                assert_eq!(attempts, 2);
                job
            }
            _ => panic!("full deferred requeue did not exhaust its bounded attempts"),
        };
        assert!(
            !budget.is_empty(),
            "deferred job still owns its reservation"
        );
        drop(rx.try_recv().unwrap());
        assert!(
            abandoned.reservation.release(),
            "final abandonment releases the last reservation"
        );
        assert!(budget.is_empty());
    }

    #[test]
    fn response_budget_charges_serialised_string_bytes() {
        let field = "\"\\\n\u{0001}";
        let serialised = serde_json::to_string(field).unwrap().len();
        assert!(serialised > field.len());

        let mut used = 0;
        assert!(charge_response_item(&mut used, &[field], 256 + field.len()).is_err());
        assert_eq!(used, 0);
        charge_response_item(&mut used, &[field], 256 + serialised).unwrap();
        assert_eq!(used, 256 + serialised);
    }
    #[test]
    fn background_retry_only_classifies_transient_admission_failures() {
        assert!(transient_background_failure(&json_error(
            "inference_busy: inference permit unavailable"
        )));
        assert!(transient_background_failure(&json_error(
            "model load failed: model loading suspended (cooldown active)"
        )));
        assert!(!transient_background_failure(&json_error(
            "validation failed: bad metadata"
        )));
    }

}
