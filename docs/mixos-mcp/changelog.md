---
title: mixos-mcp — changelog
description: Release notes of the mixos-mcp component, mirrored from cli/mixos-mcp/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from cli/mixos-mcp/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`cli/mixos-mcp/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/cli/mixos-mcp/CHANGELOG.md), where releases update it. The component's page is [mixos-mcp](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

- Retire the knowledge skills protocol, six skills tools and `knowledge_brief`,
  including LLM refinement and writes to `CLAUDE.md`. Context search no longer
  queries skills; the digest reports index statistics and staleness samples.
  Optional feedback remains consumed by indexd search ranking. Stored data is
  unchanged; the indexd manual documents filtered CMM observation deletion.
- Deployment cleanup: remove `trust_weight_graduated`, `trust_weight_skill`
  and any old `skills`/`llm` sections from `knowledge.conf.mix`; remove the
  unused `skills.conf.mix` and `llm.conf.mix` files from the MCP config directory.
  The config loader tolerates these unknown keys, but they have no effect and
  no compatibility aliases. Generated defaults omit them; existing files are
  not rewritten. A fixture test covers the loader and obsolete files.
- Preserve digest statistics if the stale query fails, returning `stale: null`
  and `stale_error`. Query explicit thresholds of 90 days, 3 retrievals,
  180 dormant days and 5 samples per bucket. Refuse non-null `domain` arguments
  because indexd cannot scope its stats or stale queries.
- Scope the documented observation purge to IDs listed from `observation`, with
  observer/domain filters and the legacy `cosmix` domain. The Bus test proves
  intersection semantics and preservation of a matching non-observation row.
- Replace the unserved application action/control tools with `app_actions`
  and `app_execute`, using the application's registry Bus commands. Forward optional
  command parameters unchanged and preserve application error messages.
- Error-text pass-through without the old `application rc=` prefix also applies
  to `app_describe`, `desktop_windows` and `desktop_window_wait` through their
  shared native helper.
- The indexd Bus-mode call timeout is now 120 seconds by default, previously
  the Bus library's 60 seconds. SSH bridge requests carry the configured timeout.
- Retry bridge failures once before delivery for every command; after delivery,
  retry only idempotent commands. Feedback and timeouts report an unknown outcome.
