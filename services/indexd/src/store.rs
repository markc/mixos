// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

pub(crate) fn content_hash(text: &str, source: &str) -> Vec<u8> {
    // FNV-1a 128-bit hash (two 64-bit passes with different seeds)
    let mut h1 = FNV_OFFSET_BASIS;
    let mut h2 = 0x6c62_272e_07bb_0142_u64; // second seed
    for byte in source
        .bytes()
        .chain(b":".iter().copied())
        .chain(text.bytes())
    {
        h1 ^= u64::from(byte);
        h1 = h1.wrapping_mul(FNV_PRIME);
        h2 ^= u64::from(byte);
        h2 = h2.wrapping_mul(0x0000_0100_0000_01c9); // different prime
    }
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&h1.to_le_bytes());
    out.extend_from_slice(&h2.to_le_bytes());
    out
}

// --- Request/Response types ---
pub(crate) struct VectorDb {
    pub(crate) conn: Connection,
    /// In-memory exact search mirror of `vec_chunks` — see `vindex.rs`
    /// for the layout and the writer/reader lock contract. Shared with
    /// the off-mutex search path via `Arc`.
    pub(crate) search_index: Arc<std::sync::RwLock<vindex::VectorIndex>>,
    pub(crate) checkpoint_owed: CheckpointOwed,
}

pub(crate) enum StoreAttempt {
    Committed { ids: Vec<i64>, duplicates: usize },
    NeedsEmbeddings(Vec<usize>),
}

#[derive(Default)]
pub(crate) struct CheckpointOwed(AtomicBool);

pub(crate) static CHECKPOINT_RETRY_NOTIFY: tokio::sync::Notify = tokio::sync::Notify::const_new();

impl CheckpointOwed {
    pub(crate) fn mark(&self) {
        self.0.store(true, Ordering::Release);
        CHECKPOINT_RETRY_NOTIFY.notify_one();
    }

    pub(crate) fn is_owed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn record_result(&self, busy: bool) {
        self.0.store(busy, Ordering::Release);
    }
}

pub(crate) fn append_metadata_filters(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filters: &[MetadataFilter],
    for_delete: bool,
) -> Result<()> {
    for filter in filters {
        if for_delete {
            anyhow::ensure!(
                !matches!(filter.op, FilterOp::Contains),
                "delete metadata filters do not allow contains"
            );
            anyhow::ensure!(
                matches!(filter.value, serde_json::Value::String(_) | serde_json::Value::Number(_))
                    || (matches!(filter.op, FilterOp::Eq) && filter.value.is_boolean()),
                "delete metadata filter requires a non-null string or number (or boolean for eq)"
            );
        }
        let idx = params.len() + 1;
        let op = match filter.op {
            FilterOp::Eq => "=",
            FilterOp::Gt => ">",
            FilterOp::Lt => "<",
            FilterOp::Gte => ">=",
            FilterOp::Lte => "<=",
            FilterOp::Contains => "LIKE",
        };
        sql.push_str(&format!(
            " AND json_extract(c.metadata, ?{idx}) {op} ?{}",
            idx + 1
        ));
        params.push(Box::new(format!("$.{}", filter.field)));
        if matches!(filter.op, FilterOp::Contains) {
            let value = filter.value.as_str().filter(|s| !s.is_empty());
            let value = value.ok_or_else(|| {
                anyhow::anyhow!("contains metadata filter requires a non-empty string")
            })?;
            let escaped = value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            sql.push_str(" ESCAPE '\\'");
            params.push(Box::new(format!("%{escaped}%")));
        } else {
            match &filter.value {
                serde_json::Value::Number(n) => params.push(Box::new(n.as_f64().unwrap_or(0.0))),
                serde_json::Value::String(s) => params.push(Box::new(s.clone())),
                serde_json::Value::Bool(b) if for_delete => params.push(Box::new(*b)),
                other => params.push(Box::new(other.to_string())),
            }
        }
    }
    Ok(())
}

impl VectorDb {
    pub(crate) fn open(
        path: &str,
        vindex_dtype: vindex::VectorDtype,
        journal_size_limit_bytes: u64,
    ) -> Result<Self> {
        // SAFETY: sqlite3_vec_init implements the SQLite extension entry ABI;
        // auto_extension records this static function pointer for future connections.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        if let Some(parent) = std::path::Path::new(path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; \
             PRAGMA synchronous=NORMAL; \
             PRAGMA busy_timeout=5000; \
             PRAGMA cache_size=-2000;",
        )?;
        let journal_limit = i64::try_from(journal_size_limit_bytes)
            .context("journal_size_limit_bytes exceeds SQLite i64 range")?;
        conn.pragma_update(None, "journal_size_limit", journal_limit)?;
        // Default new transactions to BEGIN IMMEDIATE so writers grab the write lock up
        // front (no deferred→write upgrade that can deadlock two writers under WAL).
        conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);

        // Create base tables (index on content_hash deferred — may not exist on upgraded DBs)
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS chunks (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                content       TEXT NOT NULL,
                source        TEXT NOT NULL DEFAULT '',
                metadata      TEXT NOT NULL DEFAULT '',
                content_hash  BLOB,
                created       TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(
                embedding float[{EMBEDDING_DIM}]
            );"
        ))?;

        // Migration: add content_hash column if upgrading from older schema
        let has_hash_col: bool = conn
            .prepare("SELECT content_hash FROM chunks LIMIT 0")
            .is_ok();
        if !has_hash_col {
            info!("migrating: adding content_hash column");
            conn.execute_batch(
                "ALTER TABLE chunks ADD COLUMN content_hash BLOB;
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_content_hash
                     ON chunks(content_hash) WHERE content_hash IS NOT NULL;",
            )?;
        }

        // Migration: add feedback_score column for relevance tracking
        let has_feedback_col: bool = conn
            .prepare("SELECT feedback_score FROM chunks LIMIT 0")
            .is_ok();
        if !has_feedback_col {
            info!("migrating: adding feedback_score column");
            conn.execute_batch(
                "ALTER TABLE chunks ADD COLUMN feedback_score INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        // Migration: add retrieval tracking columns (implicit negative signal + staleness)
        let has_retrieval_col: bool = conn
            .prepare("SELECT retrieval_count FROM chunks LIMIT 0")
            .is_ok();
        if !has_retrieval_col {
            info!("migrating: adding retrieval_count + last_retrieved columns");
            conn.execute_batch(
                "ALTER TABLE chunks ADD COLUMN retrieval_count INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE chunks ADD COLUMN last_retrieved TEXT;",
            )?;
        }

        // Migration: add superseded_by column for explicit chunk-supersede relationships
        // (e.g. a later journal entry that invalidates an earlier one). Chunks with
        // superseded_by IS NOT NULL are filtered out of context_search results by default
        // but remain in the database for audit/rollback.
        let has_superseded_col: bool = conn
            .prepare("SELECT superseded_by FROM chunks LIMIT 0")
            .is_ok();
        if !has_superseded_col {
            info!("migrating: adding superseded_by column");
            conn.execute_batch(
                "ALTER TABLE chunks ADD COLUMN superseded_by INTEGER DEFAULT NULL;",
            )?;
        }

        // Ensure content_hash index exists (must come AFTER content_hash migration above).
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_content_hash
                 ON chunks(content_hash) WHERE content_hash IS NOT NULL;",
        )?;

        // Candidate preselection for the in-memory search path: active
        // (non-superseded) ids by source, index-only.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_chunks_active_source
                 ON chunks(source, id) WHERE superseded_by IS NULL;",
        )?;

        // Build the in-memory search index from vec_chunks. FAIL LOUDLY
        // on any inconsistency (bad blob width, duplicate rowid,
        // chunks↔vectors id-set drift) — silently serving a partial
        // index would return wrong search results while looking healthy.
        //
        // The rollback switch must work even when the LOADER is what's
        // broken: under MIXOS_INDEXD_SEARCH=sqlite the mirror is
        // neither built nor validated, so a bad corpus can't stop the
        // daemon from starting in fallback mode.
        if sqlite_search_fallback() {
            let version: String = conn.query_row("SELECT vec_version()", [], |r| r.get(0))?;
            info!(
                "vector db opened at {path} (sqlite-vec {version}; in-memory search DISABLED via MIXOS_INDEXD_SEARCH=sqlite)"
            );
            return Ok(Self {
                conn,
                search_index: Arc::new(std::sync::RwLock::new(vindex::VectorIndex::new(
                    EMBEDDING_DIM,
                    vindex_dtype,
                ))),
                checkpoint_owed: CheckpointOwed::default(),
            });
        }
        let load_t0 = Instant::now();
        let row_count: usize = conn
            .query_row("SELECT COUNT(*) FROM vec_chunks", [], |row| {
                row.get::<_, i64>(0)
            })?
            .try_into()
            .context("negative or oversized vec_chunks row count")?;
        let mut index = vindex::VectorIndex::with_capacity(EMBEDDING_DIM, row_count, vindex_dtype);
        {
            let mut stmt = conn.prepare("SELECT rowid, embedding FROM vec_chunks")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in rows {
                let (id, blob) = row?;
                if blob.len() != EMBEDDING_DIM * 4 {
                    anyhow::bail!(
                        "vec_chunks rowid {id}: blob {} bytes, expected {}",
                        blob.len(),
                        EMBEDDING_DIM * 4
                    );
                }
                let emb: Vec<f32> = blob
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                let before = index.len();
                index.upsert(id, &emb).map_err(|e| anyhow::anyhow!(e))?;
                if index.len() == before {
                    anyhow::bail!("vec_chunks duplicate rowid {id}");
                }
            }
        }
        // Identity validation, both directions — equal COUNTs can hide
        // a missing vector plus an orphan cancelling out. Surface the
        // actual offending ids so repair is actionable.
        let missing_vecs: Vec<i64> = conn
            .prepare("SELECT id FROM chunks EXCEPT SELECT rowid FROM vec_chunks LIMIT 10")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if !missing_vecs.is_empty() {
            anyhow::bail!(
                "chunks without vectors (first 10): {missing_vecs:?} — repair before serving \
                 (or MIXOS_INDEXD_SEARCH=sqlite to boot in fallback mode)"
            );
        }
        let orphan_vecs: Vec<i64> = conn
            .prepare("SELECT rowid FROM vec_chunks EXCEPT SELECT id FROM chunks LIMIT 10")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if !orphan_vecs.is_empty() {
            anyhow::bail!(
                "orphan vectors without chunks (first 10): {orphan_vecs:?} — repair before serving \
                 (or MIXOS_INDEXD_SEARCH=sqlite to boot in fallback mode)"
            );
        }

        let version: String = conn.query_row("SELECT vec_version()", [], |r| r.get(0))?;
        info!(
            vectors = index.len(),
            load_ms = load_t0.elapsed().as_millis() as u64,
            "vector db opened at {path} (sqlite-vec {version}; in-memory search index loaded)"
        );
        Ok(Self {
            conn,
            search_index: Arc::new(std::sync::RwLock::new(index)),
            checkpoint_owed: CheckpointOwed::default(),
        })
    }

    /// Read-only optimistic duplicate probe. This deliberately does not update
    /// metadata: no durable side effect may precede successful inference and
    /// the final IMMEDIATE transaction.
    pub(crate) fn preflight_duplicate_ids(
        &self,
        texts: &[String],
        source: &str,
    ) -> Result<Vec<Option<i64>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM chunks WHERE content_hash = ?1")?;
        let ids: rusqlite::Result<Vec<_>> = texts
            .iter()
            .map(|text| {
                let hash = content_hash(text, source);
                stmt.query_row([hash], |row| row.get(0)).optional()
            })
            .collect();
        Ok(ids?)
    }

    /// Final all-or-nothing store. Every item skipped by the optimistic
    /// preflight is re-resolved after BEGIN IMMEDIATE and before the first
    /// write. If any disappeared, the transaction rolls back cleanly and the
    /// caller computes just those missing embeddings off-lock before retrying.
    pub(crate) fn store_revalidated(
        &self,
        embeddings: &[Option<Vec<f32>>],
        preflight_ids: &[Option<i64>],
        texts: &[String],
        source: &str,
        metadata: &[String],
    ) -> Result<StoreAttempt> {
        if embeddings.len() != texts.len() || preflight_ids.len() != texts.len() {
            anyhow::bail!("store input lengths do not match");
        }
        let mut ids = Vec::with_capacity(embeddings.len());
        let mut duplicates = 0usize;

        // Wrap the whole multi-chunk store in a single transaction so it's one commit
        // (and one write-lock acquisition) instead of one tx per row. The connection's
        // default transaction behavior is set to IMMEDIATE in `open()`, so this
        // `unchecked_transaction` begins with BEGIN IMMEDIATE (grabs the write lock up
        // front, avoiding a deferred→write upgrade that can deadlock under concurrency).
        // `unchecked_transaction` works on `&self`; access to this connection is
        // serialized by the AppState tokio Mutex (every caller holds the lock guard),
        // so no two transactions overlap on one connection. All embedding/CPU work is
        // already done before this call — nothing CPU-bound runs inside the tx.
        //
        // Search-index write gate: hold the write lock across commit +
        // memory patch so a concurrent search (read lock + its own read
        // transaction) sees before-state or after-state, never a mix.
        let mut vindex = self
            .search_index
            .write()
            .expect("search index RwLock poisoned");
        let mut inserted_vecs: Vec<(i64, usize)> = Vec::new();
        let tx = self.conn.unchecked_transaction()?;

        let mut vanished = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT id FROM chunks WHERE content_hash = ?1")?;
            for (i, (text, preflight_id)) in texts.iter().zip(preflight_ids).enumerate() {
                if preflight_id.is_some() && embeddings[i].is_none() {
                    let hash = content_hash(text, source);
                    if stmt
                        .query_row([hash], |row| row.get::<_, i64>(0))
                        .optional()?
                        .is_none()
                    {
                        vanished.push(i);
                    }
                }
            }
        }
        if !vanished.is_empty() {
            tx.rollback()?;
            return Ok(StoreAttempt::NeedsEmbeddings(vanished));
        }

        for (i, (embedding, text)) in embeddings.iter().zip(texts.iter()).enumerate() {
            let meta = metadata.get(i).map(|s| s.as_str()).unwrap_or("");
            let hash = content_hash(text, source);

            if embedding.is_none() {
                let existing_id: i64 = tx.query_row(
                    "SELECT id FROM chunks WHERE content_hash = ?1",
                    [&hash],
                    |r| r.get(0),
                )?;
                if !meta.is_empty() {
                    tx.execute(
                        "UPDATE chunks SET metadata = ?1 WHERE id = ?2",
                        rusqlite::params![meta, existing_id],
                    )?;
                }
                ids.push(existing_id);
                duplicates += 1;
                continue;
            }

            // INSERT OR IGNORE — unique index on content_hash rejects exact duplicates
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO chunks (content, source, metadata, content_hash) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![text, source, meta, hash],
            )?;

            if inserted == 0 {
                // Duplicate — return existing ID, optionally update metadata
                let existing_id: i64 = tx.query_row(
                    "SELECT id FROM chunks WHERE content_hash = ?1",
                    [&hash],
                    |r| r.get(0),
                )?;
                if !meta.is_empty() {
                    tx.execute(
                        "UPDATE chunks SET metadata = ?1 WHERE id = ?2",
                        rusqlite::params![meta, existing_id],
                    )?;
                }
                ids.push(existing_id);
                duplicates += 1;
                continue;
            }

            let rowid = tx.last_insert_rowid();

            let blob = vec_to_blob(embedding.as_deref().expect("embedding checked above"));
            tx.execute(
                "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![rowid, blob],
            )?;

            inserted_vecs.push((rowid, i));
            ids.push(rowid);
        }

        tx.commit()?;
        // Patch AFTER the commit succeeded; a tx error above unwinds
        // without touching the in-memory index.
        for (rowid, i) in inserted_vecs {
            if let Err(e) = vindex.upsert(
                rowid,
                embeddings[i]
                    .as_deref()
                    .expect("inserted rows always have embeddings"),
            ) {
                warn!("search index patch failed for chunk {rowid}: {e}");
            }
        }

        Ok(StoreAttempt::Committed { ids, duplicates })
    }

    pub(crate) fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        source_filter: &str,
        metadata_filters: &[MetadataFilter],
        max_response_bytes: usize,
    ) -> Result<Vec<SearchResult>> {
        let blob = vec_to_blob(query_embedding);

        // Build the base query — sqlite-vec requires MATCH + k in the WHERE clause.
        // Superseded chunks are excluded here so they cannot surface through normal
        // retrieval; they remain queryable by explicit id for audit/rollback.
        let mut sql = String::from(
            "SELECT v.rowid, v.distance, c.content, c.source, c.metadata, c.feedback_score,
                    c.retrieval_count, c.last_retrieved, c.created
             FROM vec_chunks v
             JOIN chunks c ON c.id = v.rowid
             WHERE v.embedding MATCH ?1
             AND k = ?2
             AND c.superseded_by IS NULL",
        );

        let limit = sqlite_count(limit.min(MAX_SEARCH_LIMIT), "search.limit")
            .map_err(anyhow::Error::msg)?;
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(blob), Box::new(limit)];
        let param_idx = 3;

        if !source_filter.is_empty() {
            sql.push_str(&format!(" AND c.source = ?{param_idx}"));
            params.push(Box::new(source_filter.to_string()));
        }

        append_metadata_filters(&mut sql, &mut params, metadata_filters, false)?;

        sql.push_str(" ORDER BY v.distance");

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(&*param_refs, |row| {
            Ok(SearchResult {
                id: row.get(0)?,
                distance: row.get(1)?,
                content: row.get(2)?,
                source: row.get(3)?,
                metadata: row.get(4)?,
                feedback_score: row.get::<_, i64>(5).unwrap_or(0),
                retrieval_count: row.get::<_, i64>(6).unwrap_or(0),
                last_retrieved: row.get::<_, Option<String>>(7).ok().flatten(),
                created: row.get::<_, String>(8).unwrap_or_default(),
            })
        })?;

        let mut results: Vec<SearchResult> = Vec::new();
        let mut response_bytes = 0usize;
        for row in rows {
            let result = row?;
            charge_response_item(
                &mut response_bytes,
                &[
                    &result.content,
                    &result.source,
                    &result.metadata,
                    &result.created,
                ],
                max_response_bytes,
            )?;
            results.push(result);
        }
        // Re-sort by adjusted distance: feedback boost - implicit negative - staleness penalty.
        // Lower adjusted distance = better rank.
        let now_days = days_since_epoch_utc();
        results.sort_by(|a, b| {
            let adj_a = adjusted_distance(a, now_days);
            let adj_b = adjusted_distance(b, now_days);
            adj_a
                .partial_cmp(&adj_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }

    /// Fire-and-forget: mark these chunk IDs as retrieved (increment count, update timestamp).
    /// Silently ignores errors — retrieval tracking is best-effort.
    pub(crate) fn mark_retrieved(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE chunks SET retrieval_count = retrieval_count + 1,
                               last_retrieved = datetime('now')
             WHERE id IN ({placeholders})"
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let _ = self.conn.execute(&sql, &*refs);
    }

    pub(crate) fn update(
        &self,
        id: i64,
        content: Option<&str>,
        metadata: Option<&str>,
        source: Option<&str>,
        new_embedding: Option<&[f32]>,
    ) -> Result<bool> {
        let mut set_clauses = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(c) = content {
            set_clauses.push(format!("content = ?{idx}"));
            params.push(Box::new(c.to_string()));
            idx += 1;
        }
        if let Some(m) = metadata {
            set_clauses.push(format!("metadata = ?{idx}"));
            params.push(Box::new(m.to_string()));
            idx += 1;
        }
        if let Some(s) = source {
            set_clauses.push(format!("source = ?{idx}"));
            params.push(Box::new(s.to_string()));
            idx += 1;
        }

        if set_clauses.is_empty() && new_embedding.is_none() {
            return Ok(false);
        }

        // One transaction for the row update + vector swap (previously
        // three autocommits — a crash between them could strand a chunk
        // without its vector), gated on the search-index write lock.
        let mut vindex = self
            .search_index
            .write()
            .expect("search index RwLock poisoned");
        let tx = self.conn.unchecked_transaction()?;

        // Reject a nonexistent id BEFORE touching vec_chunks: the old
        // code happily inserted a vector row for an id with no chunk (a
        // caller typo minted an orphan), which the startup identity
        // check would now turn into a boot failure.
        let exists = match tx.query_row("SELECT 1 FROM chunks WHERE id = ?1", [id], |_| Ok(())) {
            Ok(()) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => return Err(e.into()),
        };
        if !exists {
            return Ok(false);
        }

        if !set_clauses.is_empty() {
            let sql = format!(
                "UPDATE chunks SET {} WHERE id = ?{}",
                set_clauses.join(", "),
                idx
            );
            params.push(Box::new(id));
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            tx.execute(&sql, &*param_refs)?;
        }

        if let Some(emb) = new_embedding {
            let blob = vec_to_blob(emb);
            // sqlite-vec: delete old + insert new for the same rowid
            tx.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [id])?;
            tx.execute(
                "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, blob],
            )?;
        }

        tx.commit()?;
        if let Some(emb) = new_embedding
            && let Err(e) = vindex.upsert(id, emb)
        {
            warn!("search index patch failed for chunk {id}: {e}");
        }

        Ok(true)
    }

    pub(crate) fn list(
        &self,
        source_filter: &str,
        limit: i64,
        offset: i64,
        max_response_bytes: usize,
    ) -> Result<(Vec<ListItem>, usize)> {
        let has_filter = !source_filter.is_empty();

        let total: usize = if has_filter {
            self.conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE source = ?1",
                [source_filter],
                |r| r.get::<_, i64>(0).map(|v| v as usize),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(*) FROM chunks", [], |r| {
                    r.get::<_, i64>(0).map(|v| v as usize)
                })?
        };

        let sql = if has_filter {
            "SELECT id, content, source, metadata, created FROM chunks WHERE source = ?1 ORDER BY created DESC, id DESC LIMIT ?2 OFFSET ?3"
        } else {
            "SELECT id, content, source, metadata, created FROM chunks WHERE 1=1 ORDER BY created DESC, id DESC LIMIT ?1 OFFSET ?2"
        };

        let mut stmt = self.conn.prepare(sql)?;
        let mut items = Vec::new();
        let mut response_bytes = 0usize;

        if has_filter {
            let rows = stmt.query_map(rusqlite::params![source_filter, limit, offset], |row| {
                Ok(ListItem {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    source: row.get(2)?,
                    metadata: row.get(3)?,
                    created: row.get(4)?,
                })
            })?;
            for row in rows {
                let item = row?;
                charge_response_item(
                    &mut response_bytes,
                    &[&item.content, &item.source, &item.metadata, &item.created],
                    max_response_bytes,
                )?;
                items.push(item);
            }
        } else {
            let rows = stmt.query_map(rusqlite::params![limit, offset], |row| {
                Ok(ListItem {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    source: row.get(2)?,
                    metadata: row.get(3)?,
                    created: row.get(4)?,
                })
            })?;
            for row in rows {
                let item = row?;
                charge_response_item(
                    &mut response_bytes,
                    &[&item.content, &item.source, &item.metadata, &item.created],
                    max_response_bytes,
                )?;
                items.push(item);
            }
        }

        Ok((items, total))
    }

    pub(crate) fn feedback(&self, id: i64, useful: bool) -> Result<i64> {
        let delta: i64 = if useful { 1 } else { -1 };
        self.conn.execute(
            "UPDATE chunks SET feedback_score = feedback_score + ?1 WHERE id = ?2",
            rusqlite::params![delta, id],
        )?;
        let new_score: i64 = self.conn.query_row(
            "SELECT feedback_score FROM chunks WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(new_score)
    }

    /// Mark `old_id` as superseded by `new_id`. Supersede-don't-delete semantics:
    /// the old chunk stays in the database for audit/rollback but is excluded from
    /// context_search results via the `WHERE c.superseded_by IS NULL` clause.
    /// Returns the number of rows updated (0 if old_id doesn't exist, 1 on success).
    /// Guards against self-supersede and verifies new_id exists before updating.
    pub(crate) fn supersede(&self, old_id: i64, new_id: i64) -> Result<usize> {
        if old_id == new_id {
            anyhow::bail!("cannot supersede a chunk by itself (id={old_id})");
        }
        let new_exists: bool = self
            .conn
            .query_row("SELECT 1 FROM chunks WHERE id = ?1", [new_id], |_| Ok(true))
            .unwrap_or(false);
        if !new_exists {
            anyhow::bail!("new_id={new_id} does not exist in chunks table");
        }
        let updated = self.conn.execute(
            "UPDATE chunks SET superseded_by = ?1 WHERE id = ?2",
            rusqlite::params![new_id, old_id],
        )?;
        Ok(updated)
    }

    pub(crate) fn delete(&self, ids: &[i64]) -> Result<usize> {
        self.delete_filtered(ids, &[])
    }

    /// IDs and metadata predicates are intersected when both are supplied.
    pub(crate) fn delete_filtered(&self, ids: &[i64], filters: &[MetadataFilter]) -> Result<usize> {
        anyhow::ensure!(
            !ids.is_empty() || !filters.is_empty(),
            "delete requires non-empty ids or filter"
        );
        let mut deleted = 0usize;
        // Single transaction so a multi-id delete is one commit (IMMEDIATE via the
        // connection's default behavior set in `open()`), gated on the
        // search-index write lock (commit first, then memory patch).
        let mut vindex = self
            .search_index
            .write()
            .expect("search index RwLock poisoned");
        let tx = self.conn.unchecked_transaction()?;
        let batches: Vec<&[i64]> = if ids.is_empty() {
            vec![ids]
        } else {
            ids.chunks(1000).collect()
        };
        let mut selected = Vec::new();
        for batch in batches {
            let mut sql = String::from("SELECT c.id FROM chunks c WHERE 1=1");
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            if !batch.is_empty() {
                sql.push_str(" AND c.id IN (");
                sql.push_str(&vec!["?"; batch.len()].join(","));
                sql.push(')');
                params.extend(
                    batch.iter()
                        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>),
                );
            }
            append_metadata_filters(&mut sql, &mut params, filters, true)?;
            let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            selected.extend(tx
                .prepare(&sql)?
                .query_map(&*refs, |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?);
        }
        for id in &selected {
            deleted += tx.execute("DELETE FROM chunks WHERE id = ?1", [id])?;
            tx.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [id])?;
        }
        tx.commit()?;
        for id in &selected {
            vindex.remove(*id);
        }
        Ok(deleted)
    }

    /// Return staleness candidates split into 3 buckets:
    /// 1. Never retrieved AND older than `never_retrieved_age_days` days
    /// 2. Retrieved > `low_value_min_retrievals` times with feedback_score <= 0
    /// 3. Last retrieved older than `long_dormant_days` days ago
    pub(crate) fn stale_query(&self, req: &StaleRequest) -> Result<StaleResponse> {
        let total: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| {
                r.get::<_, i64>(0).map(|v| v as usize)
            })?;

        // Build optional source clause — always put source param at position 3 if present.
        let (src_clause, src_param): (&str, Option<&str>) = if req.source.is_empty() {
            ("", None)
        } else {
            (" AND source = ?3", Some(req.source.as_str()))
        };
        let limit = sqlite_count(
            clamp_stale_limit(req.per_bucket_limit),
            "stale.per_bucket_limit",
        )
        .map_err(anyhow::Error::msg)?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<StaleChunk> {
            let metadata: String = row.get(7)?;
            let (path, filename) = extract_path_fields(&metadata);
            Ok(StaleChunk {
                id: row.get(0)?,
                source: row.get(1)?,
                preview: row.get(2)?,
                retrieval_count: row.get(3)?,
                feedback_score: row.get(4)?,
                last_retrieved: row.get(5)?,
                created: row.get(6)?,
                path,
                filename,
            })
        };

        let run_query = |sql: String, p1: i64| -> Result<Vec<StaleChunk>> {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = if let Some(s) = src_param {
                stmt.query_map(rusqlite::params![p1, limit, s], map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                stmt.query_map(rusqlite::params![p1, limit], map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(rows)
        };

        let never_retrieved_old = run_query(
            format!(
                "SELECT id, source, substr(content, 1, 200), retrieval_count, feedback_score,
                        last_retrieved, created, metadata
                 FROM chunks
                 WHERE last_retrieved IS NULL
                   AND julianday('now') - julianday(created) > ?1{src_clause}
                 ORDER BY created ASC LIMIT ?2"
            ),
            req.never_retrieved_age_days,
        )?;

        let low_value = run_query(
            format!(
                "SELECT id, source, substr(content, 1, 200), retrieval_count, feedback_score,
                        last_retrieved, created, metadata
                 FROM chunks
                 WHERE retrieval_count > ?1 AND feedback_score <= 0{src_clause}
                 ORDER BY retrieval_count DESC LIMIT ?2"
            ),
            req.low_value_min_retrievals,
        )?;

        let long_dormant = run_query(
            format!(
                "SELECT id, source, substr(content, 1, 200), retrieval_count, feedback_score,
                        last_retrieved, created, metadata
                 FROM chunks
                 WHERE last_retrieved IS NOT NULL
                   AND julianday('now') - julianday(last_retrieved) > ?1{src_clause}
                 ORDER BY last_retrieved ASC LIMIT ?2"
            ),
            req.long_dormant_days,
        )?;

        Ok(StaleResponse {
            never_retrieved_old,
            low_value,
            long_dormant,
            total_chunks: total,
        })
    }

    pub(crate) fn stats(&self, db_path: &str) -> Result<StatsResponse> {
        let total: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| {
                r.get::<_, i64>(0).map(|v| v as usize)
            })?;
        let db_size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);

        let mut by_source = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT source, COUNT(*) FROM chunks GROUP BY source ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceCount {
                source: row.get(0)?,
                count: row.get::<_, i64>(1).map(|v| v as usize)?,
            })
        })?;
        for row in rows {
            by_source.push(row?);
        }

        Ok(StatsResponse {
            total_vectors: total,
            db_size_bytes: db_size,
            model_loaded: false, // caller fills runtime fields
            model_circuit: String::new(),
            embed_circuit: String::new(),
            embed_cache_entries: 0,
            embed_cache_hits: 0,
            embed_cache_misses: 0,
            queue_depth: 0,
            queued_bytes: 0,
            inference_in_flight: 0,
            timed_out_still_running: 0,
            model_generation: 0,
            rss_before_load_bytes: 0,
            rss_after_load_bytes: 0,
            rss_after_unload_bytes: 0,
            by_source,
        })
    }

    pub(crate) fn mark_checkpoint_owed(&self) {
        self.checkpoint_owed.mark();
    }

    pub(crate) fn checkpoint_truncate(&self) -> Result<()> {
        let was_owed = self.checkpoint_owed.is_owed();
        let (busy, log, checkpointed): (i64, i64, i64) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        let is_busy = busy == 1;
        self.checkpoint_owed.record_result(is_busy);
        if is_busy {
            tracing::debug!(
                was_owed,
                log_frames = log,
                checkpointed_frames = checkpointed,
                "WAL checkpoint busy; retry owed on next background drain"
            );
        }
        Ok(())
    }
}

pub(crate) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_owed_stays_sticky_until_success() {
        let owed = CheckpointOwed::default();
        assert!(!owed.is_owed());
        owed.mark();
        assert!(owed.is_owed());
        owed.record_result(true);
        assert!(owed.is_owed(), "busy checkpoint must remain owed");
        owed.record_result(false);
        assert!(!owed.is_owed(), "successful retry clears owed state");
    }
    fn test_vector_db() -> VectorDb {
        // SAFETY: sqlite3_vec_init implements the SQLite extension entry ABI;
        // auto_extension records this static function pointer for future connections.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let mut conn = Connection::open_in_memory().unwrap();
        conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        conn.execute_batch(&format!(
            "CREATE TABLE chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT '',
                metadata TEXT NOT NULL DEFAULT '',
                content_hash BLOB UNIQUE,
                created TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{EMBEDDING_DIM}]);"
        ))
        .unwrap();
        VectorDb {
            conn,
            search_index: Arc::new(std::sync::RwLock::new(vindex::VectorIndex::new(
                EMBEDDING_DIM,
                vindex::VectorDtype::F32,
            ))),
            checkpoint_owed: CheckpointOwed::default(),
        }
    }

    #[test]
    fn delete_chunks_2500_ids_in_one_transaction() {
        let db = test_vector_db();
        let texts: Vec<String> = (0..2501).map(|i| format!("chunk {i}")).collect();
        let StoreAttempt::Committed { ids, .. } = db.store_revalidated(
            &vec![Some(vec![1.0; EMBEDDING_DIM]); texts.len()],
            &vec![None; texts.len()],
            &texts,
            "doc",
            &vec!["{}".into(); texts.len()],
        ).unwrap() else {
            panic!("unexpected race")
        };
        assert_eq!(db.delete_filtered(&ids[..2500], &[]).unwrap(), 2500);
        assert_eq!(db.stats("").unwrap().total_vectors, 1);
        assert_eq!(db.search_index.read().unwrap().len(), 1);
        assert_eq!(db.conn.query_row("SELECT rowid FROM vec_chunks", [], |row| row.get::<_, i64>(0)).unwrap(), ids[2500]);
    }

    #[test]
    fn delete_refuses_contains_and_invalid_comparison_values() {
        let db = test_vector_db();
        db.conn.execute("INSERT INTO chunks(content, metadata) VALUES ('keep', '{}')", []).unwrap();
        for raw in [
            serde_json::json!({"field":"path","op":"contains","value":"a"}),
            serde_json::json!({"field":"path","op":"eq"}),
            serde_json::json!({"field":"path","op":"eq","value":null}),
            serde_json::json!({"field":"path","op":"eq","value":[]}),
            serde_json::json!({"field":"path","op":"eq","value":{}}),
            serde_json::json!({"field":"path","op":"gt","value":true}),
        ] {
            let filter: MetadataFilter = serde_json::from_value(raw).unwrap();
            let error = db.delete_filtered(&[], &[filter]).unwrap_err().to_string();
            assert!(error.contains("delete metadata filter"), "{error}");
            assert_eq!(db.stats("").unwrap().total_vectors, 1);
        }
    }

    #[test]
    fn search_contains_matches_literal_metacharacters_on_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contains.db");
        let path = path.to_str().unwrap();
        let db = VectorDb::open(path, vindex::VectorDtype::F32, 65536).unwrap();
        let texts: Vec<String> = ["a_b", "axb", "a%b", "a\\b"].into_iter().map(String::from).collect();
        let metadata: Vec<String> = texts.iter().map(|text| serde_json::json!({"path":text}).to_string()).collect();
        db.store_revalidated(
            &vec![Some(vec![1.0; EMBEDDING_DIM]); texts.len()],
            &vec![None; texts.len()],
            &texts,
            "doc",
            &metadata,
        ).unwrap();
        for needle in ["a_b", "a%b", "a\\b"] {
            let filters: Vec<MetadataFilter> = serde_json::from_value(serde_json::json!([
                {"field":"path","op":"contains","value":needle}
            ])).unwrap();
            let fallback = db.search(&vec![1.0; EMBEDDING_DIM], 10, "", &filters, BUS_BODY_LIMIT).unwrap();
            let (exact, _) = search_exact(path, &db.search_index, &vec![1.0; EMBEDDING_DIM], 10, "", &filters, BUS_BODY_LIMIT).unwrap();
            for results in [fallback, exact] {
                assert_eq!(results.len(), 1, "{needle}");
                assert_eq!(results[0].content, needle);
            }
        }
        for value in [serde_json::json!(""), serde_json::json!(7), serde_json::Value::Null] {
            let filters: Vec<MetadataFilter> = serde_json::from_value(serde_json::json!([
                {"field":"path","op":"contains","value":value}
            ])).unwrap();
            assert!(db.search(&vec![1.0; EMBEDDING_DIM], 10, "", &filters, BUS_BODY_LIMIT).is_err());
            assert!(search_exact(path, &db.search_index, &vec![1.0; EMBEDDING_DIM], 10, "", &filters, BUS_BODY_LIMIT).is_err());
        }
    }

    #[test]
    fn filtered_delete_is_atomic_and_list_ties_are_stable() {
        let db = test_vector_db();
        let texts = vec!["a".into(), "b".into(), "c".into()];
        let StoreAttempt::Committed { ids, .. } = db
            .store_revalidated(
                &vec![Some(vec![1.0; EMBEDDING_DIM]); 3],
                &[None; 3],
                &texts,
                "doc",
                &[
                    r#"{"path":"/srv/a","rank":1}"#.into(),
                    r#"{"path":"/srv/a","rank":2}"#.into(),
                    r#"{"path":"/srv/b","rank":3}"#.into(),
                ],
            )
            .unwrap()
        else {
            panic!("unexpected race")
        };
        db.conn
            .execute("UPDATE chunks SET created = '2026-10-20'", [])
            .unwrap();
        for source in ["", "doc"] {
            let (page, _) = db.list(source, 1, 1, BUS_BODY_LIMIT).unwrap();
            assert_eq!(page[0].id, ids[1]);
        }
        assert!(db.delete_filtered(&[], &[]).is_err());
        let filters: Vec<MetadataFilter> = serde_json::from_value(serde_json::json!([
            {"field":"path","op":"eq","value":"/srv/a"},
            {"field":"rank","op":"gte","value":2}
        ]))
        .unwrap();
        assert_eq!(db.delete_filtered(&[ids[0], ids[1]], &filters).unwrap(), 1);
        let filters: Vec<MetadataFilter> = serde_json::from_value(serde_json::json!([
            {"field":"path","op":"eq","value":"/srv/a"}
        ]))
        .unwrap();
        assert_eq!(db.delete_filtered(&[], &filters).unwrap(), 1);
        assert_eq!(db.delete_filtered(&[], &filters).unwrap(), 0);
        assert_eq!(
            db.conn
                .query_row("SELECT COUNT(*) FROM vec_chunks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(db.search_index.read().unwrap().len(), 1);
        let (remaining, _) = db.list("", 10, 0, BUS_BODY_LIMIT).unwrap();
        assert_eq!(remaining[0].id, ids[2]);
    }

    #[test]
    fn pre_skipped_duplicate_vanish_rolls_back_then_retries_with_embedding() {
        let db = test_vector_db();
        let texts = vec!["unchanged".to_string(), "raced".to_string()];
        let source = "doc";
        let initial_embeddings = vec![
            Some(vec![0.0; EMBEDDING_DIM]),
            Some(vec![1.0; EMBEDDING_DIM]),
        ];
        let no_preflight = vec![None, None];
        let first = db
            .store_revalidated(
                &initial_embeddings,
                &no_preflight,
                &texts,
                source,
                &["old-a".to_string(), "old-b".to_string()],
            )
            .unwrap();
        let ids = match first {
            StoreAttempt::Committed { ids, .. } => ids,
            StoreAttempt::NeedsEmbeddings(_) => panic!("initial insert unexpectedly deferred"),
        };
        let preflight = db.preflight_duplicate_ids(&texts, source).unwrap();
        assert!(preflight.iter().all(Option::is_some));
        db.delete(&[ids[1]]).unwrap();

        let raced = db
            .store_revalidated(
                &[None, None],
                &preflight,
                &texts,
                source,
                &["new-a".to_string(), "new-b".to_string()],
            )
            .unwrap();
        assert!(matches!(raced, StoreAttempt::NeedsEmbeddings(ref v) if v == &[1]));
        let metadata: String = db
            .conn
            .query_row(
                "SELECT metadata FROM chunks WHERE id = ?1",
                [ids[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metadata, "old-a", "revalidation failure wrote metadata");

        let retry = db
            .store_revalidated(
                &[None, Some(vec![1.0; EMBEDDING_DIM])],
                &preflight,
                &texts,
                source,
                &["new-a".to_string(), "new-b".to_string()],
            )
            .unwrap();
        assert!(matches!(
            retry,
            StoreAttempt::Committed { duplicates: 1, .. }
        ));
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use crate::test_support::temp_dir;

    fn schema(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn cosmix_schema_and_migrations_open_without_rewriting_data_or_schema() {
        // Register sqlite-vec, then create the production schema independently
        // of VectorDb::open. These statements are the exact cosmix base and
        // completed ad-hoc migrations (including the partial unique index).
        let _registration = VectorDb::open(":memory:", vindex::VectorDtype::F32, 65536).unwrap();
        let dir = temp_dir("cosmix-schema");
        let path = dir.join("vectors.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT '',
                metadata TEXT NOT NULL DEFAULT '',
                content_hash BLOB,
                created TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{EMBEDDING_DIM}]);
            ALTER TABLE chunks ADD COLUMN feedback_score INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE chunks ADD COLUMN retrieval_count INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE chunks ADD COLUMN last_retrieved TEXT;
            ALTER TABLE chunks ADD COLUMN superseded_by INTEGER DEFAULT NULL;
            CREATE UNIQUE INDEX idx_chunks_content_hash ON chunks(content_hash) WHERE content_hash IS NOT NULL;
            CREATE INDEX idx_chunks_active_source ON chunks(source, id) WHERE superseded_by IS NULL;"
        )).unwrap();
        let metadata = r#"{"path":"/srv/project/journal.md","date":"2026-10-18","extra":{"a":1}}"#;
        conn.execute("INSERT INTO chunks (id, content, source, metadata, content_hash, created, feedback_score, retrieval_count, last_retrieved, superseded_by) VALUES (42, 'original', 'journal', ?1, ?2, '2026-10-18 21:11:00', 7, 19, '2026-10-19 07:11:00', 43)", rusqlite::params![metadata, content_hash("original", "journal")]).unwrap();
        let original_blob = vec_to_blob(&vec![0.125; EMBEDDING_DIM]);
        conn.execute(
            "INSERT INTO vec_chunks(rowid, embedding) VALUES (42, ?1)",
            [&original_blob],
        )
        .unwrap();
        let before_schema = schema(&conn);
        let before_row: String = conn.query_row("SELECT json_array(id, content, source, metadata, hex(content_hash), created, feedback_score, retrieval_count, last_retrieved, superseded_by) FROM chunks WHERE id=42", [], |r| r.get(0)).unwrap();
        drop(conn);

        let db = VectorDb::open(path.to_str().unwrap(), vindex::VectorDtype::F32, 65536).unwrap();
        assert_eq!(schema(&db.conn), before_schema);
        let after_row: String = db.conn.query_row("SELECT json_array(id, content, source, metadata, hex(content_hash), created, feedback_score, retrieval_count, last_retrieved, superseded_by) FROM chunks WHERE id=42", [], |r| r.get(0)).unwrap();
        assert_eq!(after_row, before_row);
        let blob: Vec<u8> = db
            .conn
            .query_row("SELECT embedding FROM vec_chunks WHERE rowid=42", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(blob, original_blob);
        assert_eq!(
            db.list("journal", 10, 0, 1024 * 1024).unwrap().0[0].metadata,
            metadata
        );
        let inserted = db
            .store_revalidated(
                &[Some(vec![0.25; EMBEDDING_DIM])],
                &[None],
                &["new".into()],
                "doc",
                &["{}".into()],
            )
            .unwrap();
        let StoreAttempt::Committed { ids, .. } = inserted else {
            panic!("expected insert")
        };
        assert_eq!(ids, vec![43]);
        assert_eq!(db.feedback(42, false).unwrap(), 6);
        assert_eq!(db.delete(&ids).unwrap(), 1);
        drop(db);
        let reopened =
            VectorDb::open(path.to_str().unwrap(), vindex::VectorDtype::F32, 65536).unwrap();
        assert_eq!(reopened.search_index.read().unwrap().len(), 1);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_table_without_hash_accepts_all_ad_hoc_migrations() {
        let _registration = VectorDb::open(":memory:", vindex::VectorDtype::F32, 65536).unwrap();
        let dir = temp_dir("old-schema");
        let path = dir.join("vectors.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE chunks (id INTEGER PRIMARY KEY AUTOINCREMENT, content TEXT NOT NULL, source TEXT NOT NULL DEFAULT '', metadata TEXT NOT NULL DEFAULT '', created TEXT NOT NULL DEFAULT (datetime('now'))); CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{EMBEDDING_DIM}]);")).unwrap();
        drop(conn);
        let db = VectorDb::open(path.to_str().unwrap(), vindex::VectorDtype::F32, 65536).unwrap();
        db.conn.prepare("SELECT content_hash, feedback_score, retrieval_count, last_retrieved, superseded_by FROM chunks").unwrap();
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
