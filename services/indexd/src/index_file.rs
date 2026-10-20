// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use crate::*;

pub(crate) fn extract_path_fields(metadata: &str) -> (Option<String>, Option<String>) {
    let meta: serde_json::Value = match serde_json::from_str(metadata) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let path = meta.get("path").and_then(|v| v.as_str()).map(String::from);
    let filename = meta
        .get("filename")
        .and_then(|v| v.as_str())
        .map(String::from);
    (path, filename)
}

/// Quick YYYY-MM-DD validator (no external date crate needed).
pub(crate) fn is_valid_ymd(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let year: u32 = match s[0..4].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let month: u32 = match s[5..7].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let day: u32 = match s[8..10].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    (1970..=9999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Validate source type + required metadata fields. Enforces layer separation:
/// each source type has a distinct contract declared in the indexd config
/// TOML loaded at startup (path resolved per [`Cli::config`]). Adding a new
/// source type is a TOML edit plus `systemctl restart cosmix-indexd` — no
/// rebuild required.
///
/// Rules remain in Rust for mechanisms (date-format check, JSON parsing);
/// the TOML declares policy (which source types are accepted, which
pub(crate) const MAX_CHUNK_CHARS: usize = 6000;

pub(crate) struct ChunkTokenizer {
    pub(crate) tokenizer: Tokenizer,
    pub(crate) max_seq_len: usize,
}

impl ChunkTokenizer {
    pub(crate) fn load(model_dir: &str, configured_max_seq_len: usize, max_forward_tokens: usize) -> Result<Self> {
        // Load only config.json + tokenizer.json. This path must never touch
        // model.safetensors: token-aware file chunking is needed before the
        // first embed and must not force a full model allocation.
        let config_path = Path::new(model_dir).join("config.json");
        let tokenizer_path = Path::new(model_dir).join("tokenizer.json");        let config: nomic_bert::Config = serde_json::from_str(
            &std::fs::read_to_string(&config_path).context("reading chunker config.json")?,
        )?;
        let mut tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow::anyhow!("disabling chunker truncation: {e}"))?;
        Ok(Self {
            tokenizer,
            max_seq_len: effective_sequence_limit(
                configured_max_seq_len,
                max_forward_tokens,
                config.n_positions,
            )?,
        })
    }

    pub(crate) fn count_document_tokens(&self, text: &str) -> Result<usize, String> {
        self.tokenizer
            .encode(format!("search_document: {text}"), true)
            .map(|encoding| encoding.get_ids().len())
            .map_err(|e| format!("tokenizing index_file chunk: {e}"))
    }
}

pub(crate) async fn chunk_tokenizer(state: &Arc<Mutex<AppState>>) -> Result<Arc<ChunkTokenizer>, String> {
    let mut guard = state.lock().await;
    if let Some(tokenizer) = &guard.chunk_tokenizer {
        return Ok(tokenizer.clone());
    }
    let svc = guard.config.service.clone();
    let tokenizer = tokio::task::spawn_blocking(move || {
        ChunkTokenizer::load(&svc.model_dir, svc.max_sequence_tokens, svc.max_forward_tokens)
    }).await.map_err(|e| format!("chunk tokenizer worker failed: {e}"))?
        .map_err(|e| format!("loading tokenizer for index_file chunking: {e:#}"))?;
    let tokenizer = Arc::new(tokenizer);
    guard.chunk_tokenizer = Some(tokenizer.clone());
    Ok(tokenizer)
}
/// Split one char-bounded piece further until the tokenizer (including the
/// real document prefix and special tokens) reports every output at or below
/// the effective model sequence limit. The initial count is the fast path;
/// binary search only runs for token-dense content.
pub(crate) fn split_token_aware<F>(
    text: &str,
    max_tokens: usize,
    token_count: F,
) -> Result<Vec<String>, String>
where
    F: Fn(&str) -> Result<usize, String>,
{
    if token_count(text)? <= max_tokens {
        return Ok(vec![text.to_string()]);
    }

    let mut boundaries: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
    boundaries.push(text.len());
    let char_count = boundaries.len().saturating_sub(1);
    let mut start = 0usize;
    let mut pieces = Vec::new();
    while start < char_count {
        let mut low = start + 1;
        let mut high = char_count;
        let mut best = None;
        while low <= high {
            let mid = low + (high - low) / 2;
            let candidate = &text[boundaries[start]..boundaries[mid]];
            if token_count(candidate)? <= max_tokens {
                best = Some(mid);
                low = mid + 1;
            } else {
                high = mid.saturating_sub(1);
            }
        }
        let end = best.ok_or_else(|| {
            format!(
                "one input character plus embedding prefix exceeds the {max_tokens}-token limit"
            )
        })?;
        let piece = text[boundaries[start]..boundaries[end]].to_string();
        if token_count(&piece)? > max_tokens {
            return Err("token-aware chunker produced an oversized piece".to_string());
        }
        pieces.push(piece);
        start = end;
    }
    Ok(pieces)
}

/// Split a section's text into chunks no larger than `max_chars`, breaking at
/// line boundaries so words and markdown stay intact. A single line that is
/// itself longer than `max_chars` (a long URL, base64 blob, minified JSON, a
/// wide table row) is hard-split on char (UTF-8) boundaries so its tail is
/// never silently lost. Returns one element (the original text) when it fits.
///
/// Guarantee: every content character of the input appears in some output
/// chunk, in order — no content is dropped or truncated. The chunk *boundaries*
/// normalize whitespace (the single `\n` separating a buffered line-group from
/// the next chunk, and the newlines around a hard-split overlong line, are not
/// re-emitted), so the pieces are independent search units, not a byte-exact
/// re-segmentation of the source.
pub(crate) fn split_oversized(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize; // running char count of `cur`, kept to avoid O(n^2)
    for line in text.lines() {
        let line_len = line.chars().count();

        // An overlong single line can't fit any line-boundary chunk: flush the
        // buffer, then hard-split the line on char boundaries (UTF-8 safe — we
        // iterate `chars()`, never byte offsets, so no codepoint is ever split).
        if line_len > max_chars {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            let mut piece = String::new();
            let mut plen = 0usize;
            for ch in line.chars() {
                if plen == max_chars {
                    out.push(std::mem::take(&mut piece));
                    plen = 0;
                }
                piece.push(ch);
                plen += 1;
            }
            if !piece.is_empty() {
                out.push(piece);
            }
            continue;
        }

        if cur_len > 0 && cur_len + line_len + 1 > max_chars {
            out.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push('\n');
            cur_len += 1;
        }
        cur.push_str(line);
        cur_len += line_len;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// True when `s` is shaped like a `YYYY-MM-DD` date prefix: ten chars, a digit
/// year, and hyphens at positions 4 and 7. Month/day *range* validity is NOT
/// checked (that is [`is_valid_ymd`]'s job). This distinguishes "the author
/// intended a dated entry here" — keep it a journal so a malformed date
/// (`2026-13-01-x.md`) surfaces as a validation error — from "no date at all"
/// (`README.md`, `notes.md`) which is downgraded to a plain doc.
pub(crate) fn is_date_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit)
}

pub(crate) fn read_open_file_bounded(mut file: std::fs::File, max_bytes: usize) -> Result<String, String> {
    let metadata = file.metadata().map_err(|e| format!("fstat failed: {e}"))?;
    if !metadata.is_file() {
        return Err("path is not a regular file".to_string());
    }
    if metadata.len() > max_bytes as u64 {
        return Err(format!(
            "file is {} bytes (max {max_bytes})",
            metadata.len()
        ));
    }

    // The handle may grow after fstat. Read one byte beyond the cap from this
    // same handle and reject on observation, so neither growth nor a path swap
    // can bypass the bound.
    let read_limit = max_bytes.saturating_add(1) as u64;
    let mut bytes = Vec::with_capacity(max_bytes.saturating_add(1));
    file.by_ref()
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read failed: {e}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "file exceeded max {max_bytes} bytes while being read"
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("read failed: file is not UTF-8: {e}"))
}

pub(crate) fn read_file_bounded(path: &Path, max_bytes: usize) -> Result<String, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    read_open_file_bounded(file, max_bytes)
}

/// Index a single markdown file: read, split on ## headings, delete old entries, store new ones.
/// Called from git post-commit hooks via Bus: `indexd.index_file {"path": "/path/to/file.md"}`
pub(crate) async fn handle_index_file(
    req: IndexFileRequest,
    state: &Arc<Mutex<AppState>>,
    activity_tx: &tokio::sync::mpsc::Sender<()>,
    t: &mut ReqTiming,
) -> String {
    let config = state.lock().await.config.clone();
    let t_start = std::time::Instant::now();
    let filepath = expand_home(&req.path);
    let path = std::path::Path::new(&filepath);

    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("?");

    // Auto-detect source from path
    let source = if !req.source.is_empty() {
        req.source.clone()
    } else if filepath.contains("/_doc/") {
        "doc".to_string()
    } else if filepath.contains("/_journal/") {
        "journal".to_string()
    } else {
        "doc".to_string()
    };

    // Auto-detect domain from path via settings [domains.map], fall back to "general"
    let domain = if !req.domain.is_empty() {
        req.domain.clone()
    } else {
        config.domains.resolve(path).unwrap_or_else(|| "general".to_string())
    };

    // Obtain content: caller-supplied bytes take precedence (the only way
    // to index files the sandboxed daemon cannot read — see the `content`
    // field doc); otherwise read `path` from disk.
    let max_file_bytes = config.service.max_file_bytes;
    let content = match req.content {
        Some(c) => {
            if c.len() > max_file_bytes {
                return json_error(&format!(
                    "caller-supplied content is {} bytes (max {max_file_bytes})",
                    c.len()
                ));
            }
            c
        }
        None => match read_file_bounded(path, max_file_bytes) {
            Ok(c) => c,
            Err(e) => return json_error(&e),
        },
    };

    // Split on ## headings
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_title = filename.strip_suffix(".md").unwrap_or(filename).to_string();
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if !current_lines.is_empty() {
                let text = current_lines.join("\n");
                let text = text.trim();
                if text.len() > 50 {
                    sections.push((current_title.clone(), text.to_string()));
                }
            }
            current_title = rest.trim().to_string();
            current_lines = vec![line];
        } else {
            current_lines.push(line);
        }
    }
    // Last section
    if !current_lines.is_empty() {
        let text = current_lines.join("\n");
        let text = text.trim();
        if text.len() > 50 {
            sections.push((current_title, text.to_string()));
        }
    }

    // R3: delete-AFTER-successful-store, not before. Capture this path's
    // existing chunk ids now, but do NOT delete yet — if a later section's
    // embed/store fails, the previous good copy must survive (a delete-first
    // approach loses the doc until a successful re-index). metadata holds a
    // JSON object with a "path" field.
    let old_ids: Vec<i64> = {
        let guard = lock_timed(state, t).await;
        let db_t0 = Instant::now();
        let ids = guard
            .db
            .conn
            .prepare("SELECT id FROM chunks WHERE json_extract(metadata, '$.path') = ?1")
            .and_then(|mut stmt| {
                stmt.query_map([&filepath], |row| row.get(0))
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();
        t.db_us += db_t0.elapsed().as_micros() as u64;
        ids
    };

    if sections.is_empty() {
        // File emptied / shrunk below the section threshold ⇒ purge all of its
        // stale chunks (there is nothing to store, so delete-after collapses to
        // delete-all).
        if !old_ids.is_empty() {
            let mut guard = lock_timed(state, t).await;
            let db_t0 = Instant::now();
            match guard.db.delete(&old_ids) {
                Ok(_) => {}
                Err(e) => warn!(file = %filename, "stale-chunk purge failed: {e}"),
            }
            t.db_us += db_t0.elapsed().as_micros() as u64;
        }
        info!(file = %filename, path = %filepath, "indexed 0 sections (empty/below-threshold)");
        return serde_json::json!({"indexed": true, "sections": 0, "file": filename}).to_string();
    }

    // Keep the 6000-char heuristic as the fast first pass, then count with a
    // tokenizer-only helper and split token-dense pieces further. The helper
    // loads no model weights, so chunking cannot allocate the model merely to
    // discover boundaries. Split pieces get a "(part i/n)" title suffix.
    let tokenizer = match chunk_tokenizer(state).await {
        Ok(tokenizer) => tokenizer,
        Err(e) => return json_error(&e),
    };
    let mut token_bounded_sections = Vec::new();
    for (title, text) in sections {
        let mut pieces = Vec::new();
        for char_piece in split_oversized(&text, MAX_CHUNK_CHARS) {
            match split_token_aware(&char_piece, tokenizer.max_seq_len, |piece| {
                tokenizer.count_document_tokens(piece)
            }) {
                Ok(mut bounded) => pieces.append(&mut bounded),
                Err(e) => return json_error(&e),
            }
        }
        let n = pieces.len();
        if n == 1 {
            token_bounded_sections.push((title, pieces.remove(0)));
        } else {
            token_bounded_sections.extend(
                pieces
                    .into_iter()
                    .enumerate()
                    .map(|(i, piece)| (format!("{title} (part {}/{n})", i + 1), piece)),
            );
        }
    }
    let sections = token_bounded_sections;

    // Bound the per-request embedding workload on the FINAL chunk list
    // (post sub-split): each chunk is embedded sequentially via
    // handle_store with a 1-element batch, so an oversized document would
    // otherwise monopolise the embedding worker and bypass
    // MAX_TEXTS_PER_REQUEST. Checked here (not before the sub-split) so a
    // few huge `##` sections that explode into many chunks are also caught.
    if sections.len() > MAX_INDEX_FILE_SECTIONS {
        return json_error(&format!(
            "file splits into too many chunks: {} (max {MAX_INDEX_FILE_SECTIONS})",
            sections.len()
        ));
    }

    // Extract date from filename (YYYY-MM-DD-title.md convention). A file under
    // _journal/ whose name is NOT shaped like a dated entry (e.g. a README or
    // notes file inside a journal artifacts dir) is documentation, not a dated
    // journal entry — when the source was auto-detected, index it as a doc
    // instead of rejecting it for a missing date. A name that IS date-shaped but
    // invalid (a typo like 2026-13-01-x.md) is deliberately left as a journal so
    // validate_store_entry surfaces the error rather than silently masking it.
    // An explicit caller-supplied source is also left intact (its contract is
    // honored, error and all).
    let date = filename.get(..10).unwrap_or("");
    let (source, date) = if req.source.is_empty() && source == "journal" && !is_date_shaped(date) {
        ("doc".to_string(), "")
    } else {
        (source, date)
    };

    // Store each section sequentially (per-section embed via handle_store —
    // NOT a single batched model.embed: the padding approach regressed large
    // docs and was reverted in 0.3.1). A store failure (e.g. metadata
    // validation) is a real error, not a silent skip: surface the first one
    // with rc>=10 so the caller learns indexing did not fully succeed, rather
    // than a misleading {"indexed":true}.
    //
    // R3: collect the ids each section's StoreResponse returns into
    // `kept_ids` — `ids` carries both newly-inserted AND dedup-matched-existing
    // ids, so an unchanged section dedups to an existing id that lands in
    // kept_ids and is therefore preserved by the delete-after step below. On a
    // structural failure we return WITHOUT deleting anything, leaving the
    // previous good copy intact.
    let total = sections.len();
    let mut kept_ids: Vec<i64> = Vec::new();
    for (idx, (title, text)) in sections.iter().enumerate() {
        let meta = serde_json::json!({
            "path": filepath,
            "filename": filename,
            "section": title,
            "domain": domain,
            "type": source,
            "date": date,
        });

        let store_req = StoreRequest {
            texts: vec![text.clone()],
            source: source.clone(),
            metadata: vec![meta.to_string()],
        };
        let resp = handle_store(store_req, state, activity_tx, t).await;
        // Detect a store failure structurally (top-level `error` field),
        // not by substring: a successful StoreResponse never carries one,
        // so this can't false-positive on data that merely contains the
        // token "error".
        let parsed = serde_json::from_str::<serde_json::Value>(&resp).ok();
        let store_failed = parsed.as_ref().is_some_and(|v| v.get("error").is_some());
        if store_failed {
            // Failure ⇒ old chunks survive (old_ids were never deleted), so a
            // re-run after the cause is fixed reconciles cleanly. The caller
            // (record_request) emits the structured warn carrying this
            // detail — no separate warn here, it would double-report.
            // `work` = sections actually completed before the failure, not
            // whatever the last section's handle_store happened to leave.
            t.work = idx as u64;
            let detail = format!(
                "index_file {filename}: stored {idx}/{total} sections, then section \
                 \"{title}\" failed: {resp}"
            );
            return json_error(&detail);
        }
        // Accumulate the ids this section stored or dedup-matched.
        if let Some(ids) = parsed
            .as_ref()
            .and_then(|v| v.get("ids"))
            .and_then(|v| v.as_array())
        {
            kept_ids.extend(ids.iter().filter_map(|i| i.as_i64()));
        }
    }

    // R3: now that every section stored successfully, delete the old chunks
    // that the new content did NOT keep (sections that were removed or whose
    // text changed). Unchanged sections dedup to an existing id which is in
    // kept_ids, so they are preserved; changed/removed sections leave a stale
    // id absent from kept_ids and are purged. During the re-index window old+new
    // chunks coexist and MAY both surface in results; that transient
    // duplication is accepted as strictly safer than delete-first.
    let to_delete: Vec<i64> = old_ids
        .into_iter()
        .filter(|id| !kept_ids.contains(id))
        .collect();
    if !to_delete.is_empty() {
        let mut guard = lock_timed(state, t).await;
        let db_t0 = Instant::now();
        match guard.db.delete(&to_delete) {
            Ok(_) => {}
            // Old chunks survive; the next successful re-index purges
            // them. Not silent any more — a stuck purge is visible.
            Err(e) => warn!(file = %filename, "old-chunk purge failed after re-index: {e}"),
        }
        t.db_us += db_t0.elapsed().as_micros() as u64;
    }

    // Per-section handle_store calls set `work` to their own counts;
    // for the whole file the meaningful dimension is sections indexed.
    t.work = total as u64;

    info!(
        file = %filename,
        path = %filepath,
        sections = total,
        elapsed_ms = t_start.elapsed().as_millis() as u64,
        "indexed"
    );
    serde_json::json!({
        "indexed": true,
        "file": filename,
        "sections": total,
        "domain": domain,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;
    #[test]
    fn bounded_reader_rejects_non_regular_handles() {
        let error = read_file_bounded(Path::new("/dev/null"), 1024).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn fifo_open_is_nonblocking_and_rejected() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("input.fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let started = Instant::now();
        let error = read_file_bounded(&fifo, 1024).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn split_oversized_passthrough_when_fits() {
        let s = "line one\nline two\nline three";
        let out = split_oversized(s, 6000);
        assert_eq!(out, vec![s.to_string()]);
    }

    #[test]
    fn split_oversized_breaks_at_line_boundaries() {
        // Five 20-char lines, budget 50 → groups of 2 lines (~41 chars) per piece.
        let line = "x".repeat(20);
        let text = std::iter::repeat_n(line.as_str(), 5)
            .collect::<Vec<_>>()
            .join("\n");
        let out = split_oversized(&text, 50);
        assert!(out.len() > 1, "expected multiple pieces, got {}", out.len());
        // No piece exceeds the budget.
        for piece in &out {
            assert!(piece.chars().count() <= 50, "piece too big: {piece:?}");
        }
        // No line is ever split mid-content.
        for piece in &out {
            for l in piece.lines() {
                assert_eq!(l, line);
            }
        }
        // Reassembly is lossless (pieces rejoin to the original).
        assert_eq!(out.join("\n"), text);
    }

    #[test]
    fn split_oversized_hard_splits_overlong_single_line() {
        // A single line longer than the budget is hard-split on char boundaries
        // so its tail is not silently lost; concatenation is lossless.
        let huge = "y".repeat(100);
        let out = split_oversized(&huge, 50);
        assert!(out.len() >= 2, "expected a hard split, got {}", out.len());
        for piece in &out {
            assert!(piece.chars().count() <= 50);
        }
        assert_eq!(out.concat(), huge);
    }

    #[test]
    fn split_oversized_hard_split_is_utf8_safe() {
        // Multibyte chars must never be split mid-codepoint (would panic / corrupt).
        let s = "é".repeat(80); // 80 codepoints, 2 bytes each
        let out = split_oversized(&s, 30);
        for piece in &out {
            assert!(piece.chars().count() <= 30);
        }
        assert_eq!(out.concat(), s);
    }

    #[test]
    fn split_oversized_mixed_lines_drops_no_content() {
        // Normal short lines followed by an overlong line: every content char
        // must survive (chunk boundaries normalize separator newlines only).
        let long = "z".repeat(120);
        let text = format!("short one\nshort two\n{long}\ntail");
        let out = split_oversized(&text, 50);
        for piece in &out {
            assert!(piece.chars().count() <= 50, "piece too big: {piece:?}");
        }
        // No non-newline character is lost or reordered.
        assert_eq!(
            out.concat().replace('\n', ""),
            text.replace('\n', ""),
            "content must be preserved across mixed line-boundary + hard splits"
        );
    }

    #[test]
    fn token_aware_split_preserves_dense_tail_and_bounds_every_piece() {
        let text = "界".repeat(37);
        let count = |piece: &str| Ok(2 + piece.chars().count() * 2);
        let out = split_token_aware(&text, 16, count).unwrap();
        assert!(out.len() > 1);
        assert_eq!(out.concat(), text);
        for piece in out {
            assert!(count(&piece).unwrap() <= 16, "oversized piece {piece:?}");
        }
    }

    #[test]
    fn bounded_file_read_uses_open_handle_and_rejects_oversize() {
        let dir = temp_dir("bounded-read");
        let path = dir.join("doc.md");
        let replacement = dir.join("replacement.md");
        std::fs::write(&path, "original").unwrap();
        let opened = std::fs::File::open(&path).unwrap();
        std::fs::write(&replacement, "replacement path content").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(read_open_file_bounded(opened, 8).unwrap(), "original");

        let oversized = std::fs::File::open(&path).unwrap();
        assert!(
            read_open_file_bounded(oversized, 8)
                .unwrap_err()
                .contains("max 8")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn date_shaped_vs_valid() {
        // README/notes → not date-shaped → would downgrade to doc.
        assert!(!is_date_shaped("README.md"));
        assert!(!is_date_shaped("notes.md"));
        assert!(!is_date_shaped(""));
        // A real dated entry → date-shaped AND valid.
        assert!(is_date_shaped("2026-06-07"));
        assert!(is_valid_ymd("2026-06-07"));
        // A typo'd date → still date-shaped (stays journal) but NOT valid, so
        // validate_store_entry will surface the error instead of masking it.
        assert!(is_date_shaped("2026-13-01"));
        assert!(!is_valid_ymd("2026-13-01"));
        assert!(is_date_shaped("2026-06-aa")); // digit year + delimiters
        assert!(!is_valid_ymd("2026-06-aa"));
}
