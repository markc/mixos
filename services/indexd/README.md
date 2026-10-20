<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# indexd

Local nomic-bert embeddings, SQLite vector storage and semantic retrieval.
See the [manual](../../docs/indexd/README.md).

The transport-independent `Engine` exposes JSON methods for all eleven verbs.
`indexd check-config` validates settings; `indexd serve` opens the store and
exits with a notice until stage 4 adds the Bus loop. Neither loads model
weights. Model loading uses a local `service.model_dir`, with no downloader.

Unit tests use synthetic weights and throwaway databases. The tokenizer parity
test is ignored unless `INDEXD_TEST_TOKENIZER` is set when Cargo builds tests.
Stage 7 generates `tests/tokenizer_golden.json` from the pinned predecessor build
using `tests/tokenizer_corpus.txt` (one input per line, excluding the SPDX line).
The parity test must fail if enabled before that golden is supplied.
