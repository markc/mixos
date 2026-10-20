---
title: The semantic index service
description: How indexd embeds text locally with nomic-embed-text-v1.5 and answers vector search over the Bus, so agents can find relevant skills, docs and journals. Being ported from cosmix-indexd.
---

# The semantic index service

> **Status: being ported.** This page describes the target design. The port
> plan is `_plan/2026-10-20-indexd-port.md` in the planning hub. Only a
> dependency probe exists so far. The full verb reference arrives with
> stage 4.

`indexd` stores text chunks with their embeddings and returns the nearest
matches for a query. Embeddings are computed locally on the CPU with
`nomic-ai/nomic-embed-text-v1.5` (768 dimensions), through the pure-Rust
`candle` stack. The model files are loaded from a local directory and never
downloaded at run time. Vectors live in SQLite (`sqlite-vec`), and search runs
over an in-memory copy, re-ranked by feedback and staleness.

It runs on one workstation host as a system service, registered on that
host's own node. Agents usually reach it through `mixos-mcp`
(`context_search`, `index_workspace`, the `skills_*` tools). A desk on the
same machine reaches it over the Bus, or over ssh through `indexd bridge`,
which reads newline-delimited JSON on stdin and makes each request as a local
Bus call.

## Asking it

On the Bus, the service is `indexd`:

```mix
send indexd indexd.stats
send indexd indexd.search query="how is SRS set up on rca" limit=3
```

## Indexd is not exhaustive

indexd answers "what is relevant to X", quickly and approximately, from an
index that can lag the files. To prove that something does **not** occur, use
an exact search over the current files instead.
